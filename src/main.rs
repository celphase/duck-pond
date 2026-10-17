//! Plays a live duck pond forever.
//!
//! A background thread runs the pond simulation (see `scene.rs`) in small
//! blocks, rendering each duck's calls from where it is at that moment, and
//! feeds the audio device. With `--visualize`, a map of the pond shows where
//! every duck is relative to you, in step with what you hear. With
//! `--headphones`, each ear hears the ducks as it would on the bank, a little
//! sooner or later and brighter or duller than the other.

mod visual;

use std::collections::VecDeque;
use std::process::exit;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use rand::SeedableRng;
use rand::rngs::StdRng;

use duck_pond::scene::{Command, Scene};
use visual::Timeline;

/// Frames per simulation step and per chunk sent to the audio thread.
const BLOCK: usize = 512;
/// Chunks rendered ahead of playback (about 85 ms at 48 kHz).
const QUEUED: usize = 8;

const USAGE: &str = "usage: duck-pond [--visualize] [--headphones] [--seed N]

  -v, --visualize   show a live map of where the ducks are around you;
                    press p to scatter peas for them
  -H, --headphones  render for headphones, so the ducks sound out around
                    you rather than inside your head
  -s, --seed N      repeatable pond (default: different every run)";

type Chunk = Vec<[f32; 2]>;

struct Options {
    visualize: bool,
    headphones: bool,
    seed: Option<u64>,
}

fn parse_args() -> Options {
    let mut options = Options { visualize: false, headphones: false, seed: None };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-v" | "--visualize" => options.visualize = true,
            "-H" | "--headphones" => options.headphones = true,
            "-s" | "--seed" => match args.next().and_then(|s| s.parse().ok()) {
                Some(seed) => options.seed = Some(seed),
                None => {
                    eprintln!("--seed needs a whole number\n\n{USAGE}");
                    exit(2);
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                exit(0);
            }
            other => {
                eprintln!("unknown argument {other:?}\n\n{USAGE}");
                exit(2);
            }
        }
    }
    options
}

/// Runs the pond forever, blocking whenever `QUEUED` chunks are waiting.
fn render_forever(mut scene: Scene, tx: SyncSender<Chunk>, timeline: Option<Timeline>, commands: Receiver<Command>) {
    loop {
        while let Ok(command) = commands.try_recv() {
            scene.handle(command);
        }
        if let Some(timeline) = &timeline {
            let mut timeline = timeline.lock().unwrap();
            timeline.push_back(scene.snapshot());
            // the visualizer drains this; cap it in case it falls behind
            if timeline.len() > 1000 {
                timeline.pop_front();
            }
        }
        let mut chunk = vec![[0.0; 2]; BLOCK];
        scene.render(&mut chunk);
        if tx.send(chunk).is_err() {
            return;
        }
    }
}

fn play<T>(device: &cpal::Device, config: StreamConfig, rx: Receiver<Chunk>, played: Arc<AtomicU64>) -> cpal::Stream
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut current: Chunk = Vec::new();
    let mut pos = 0;
    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let mut consumed = 0;
        for frame in data.chunks_mut(channels) {
            if pos >= current.len() {
                match rx.try_recv() {
                    Ok(next) => (current, pos) = (next, 0),
                    // the renderer fell behind (or is just starting)
                    Err(_) => {
                        frame.fill(T::EQUILIBRIUM);
                        continue;
                    }
                }
            }
            let [l, r] = current[pos];
            pos += 1;
            consumed += 1;
            if channels == 1 {
                frame[0] = T::from_sample(0.5 * (l + r));
            } else {
                frame[0] = T::from_sample(l);
                frame[1] = T::from_sample(r);
                frame[2..].fill(T::EQUILIBRIUM);
            }
        }
        played.fetch_add(consumed, Ordering::Relaxed);
    };
    let stream = device
        .build_output_stream(config, callback, |err| eprintln!("audio error: {err}"), None)
        .expect("failed to open audio stream");
    stream.play().expect("failed to start audio stream");
    stream
}

fn main() {
    let options = parse_args();
    let rng = match options.seed {
        Some(seed) => StdRng::seed_from_u64(seed),
        None => StdRng::from_os_rng(),
    };

    let host = cpal::default_host();
    let device = host.default_output_device().expect("no audio output device");
    let supported = device.default_output_config().expect("no usable output config");
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let sr = config.sample_rate as f64;
    assert!(sr >= 16000.0, "the pond needs a sample rate of at least 16 kHz, got {sr}");
    // mixed down to one channel, the ears' timing differences would only colour the sound
    let headphones = options.headphones && config.channels >= 2;
    if options.headphones && !headphones {
        eprintln!("--headphones needs a stereo output, and this one has a single channel; playing without it");
    }
    if !options.visualize {
        let mode = if headphones { " for headphones" } else { "" };
        eprintln!("playing the duck pond{mode} at {sr} Hz, {} channel(s); Ctrl+C to stop", config.channels);
    }

    let timeline: Option<Timeline> = options.visualize.then(|| Arc::new(Mutex::new(VecDeque::new())));
    let played = Arc::new(AtomicU64::new(0));
    let (tx, rx) = sync_channel(QUEUED);
    let (command_tx, command_rx) = channel();
    let scene = Scene::new(sr, rng, headphones);
    let render_timeline = timeline.clone();
    thread::spawn(move || render_forever(scene, tx, render_timeline, command_rx));

    let audio_played = played.clone();
    let _stream = match format {
        SampleFormat::F32 => play::<f32>(&device, config, rx, audio_played),
        SampleFormat::F64 => play::<f64>(&device, config, rx, audio_played),
        SampleFormat::I16 => play::<i16>(&device, config, rx, audio_played),
        SampleFormat::I32 => play::<i32>(&device, config, rx, audio_played),
        SampleFormat::U16 => play::<u16>(&device, config, rx, audio_played),
        SampleFormat::U8 => play::<u8>(&device, config, rx, audio_played),
        other => panic!("unsupported sample format {other}"),
    };

    match timeline {
        Some(timeline) => {
            if let Err(err) = visual::run(timeline, played, command_tx) {
                eprintln!("visualizer failed: {err}");
                exit(1);
            }
        }
        None => loop {
            thread::park();
        },
    }
}
