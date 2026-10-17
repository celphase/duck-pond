//! The duck pond for the browser: a bare WebAssembly module, with no JS glue,
//! that `www/pond-worker.js` drives from a Web Worker.
//!
//! The worker creates the pond with `pond_new`, then for each block asks for
//! a snapshot of the scene (as JSON, for the map) and renders the block's
//! audio, exactly as the native player's scene thread does.

use std::cell::RefCell;
use std::fmt::Write;

use duck_pond::scene::{Command, Kind, Scene};
use rand::SeedableRng;
use rand::rngs::StdRng;

/// Frames per simulation step and per chunk sent to the audio thread.
const BLOCK: usize = 512;

struct Pond {
    scene: Scene,
    /// The last block rendered, left channel then right.
    audio: Vec<f32>,
    frames: Vec<[f32; 2]>,
    snapshot: String,
}

thread_local! {
    static POND: RefCell<Option<Pond>> = const { RefCell::new(None) };
}

fn with_pond<T>(f: impl FnOnce(&mut Pond) -> T) -> T {
    POND.with_borrow_mut(|pond| f(pond.as_mut().expect("pond_new has not been called")))
}

/// Frames in each block `pond_render` makes.
#[unsafe(no_mangle)]
pub extern "C" fn pond_block() -> u32 {
    BLOCK as u32
}

/// Fill the pond with ducks, to be rendered at `sample_rate` (at least
/// 16 kHz). The seed is split into two halves, as JS numbers can't hold it.
#[unsafe(no_mangle)]
pub extern "C" fn pond_new(sample_rate: f64, seed_hi: u32, seed_lo: u32, headphones: u32) {
    let seed = (seed_hi as u64) << 32 | seed_lo as u64;
    let scene = Scene::new(sample_rate.max(16000.0), StdRng::seed_from_u64(seed), headphones != 0);
    let pond = Pond { scene, audio: vec![0.0; 2 * BLOCK], frames: vec![[0.0; 2]; BLOCK], snapshot: String::new() };
    POND.set(Some(pond));
}

#[unsafe(no_mangle)]
pub extern "C" fn pond_scatter_peas() {
    with_pond(|pond| pond.scene.handle(Command::ScatterPeas));
}

#[unsafe(no_mangle)]
pub extern "C" fn pond_set_headphones(headphones: u32) {
    with_pond(|pond| pond.scene.set_headphones(headphones != 0));
}

/// Render the next block and return where it is: `pond_block()` frames for
/// the left channel, then as many for the right.
#[unsafe(no_mangle)]
pub extern "C" fn pond_render() -> *const f32 {
    with_pond(|pond| {
        pond.scene.render(&mut pond.frames);
        let (left, right) = pond.audio.split_at_mut(BLOCK);
        for (k, [l, r]) in pond.frames.iter().enumerate() {
            (left[k], right[k]) = (*l, *r);
        }
        pond.audio.as_ptr()
    })
}

/// Describe the scene as it is about to be heard as JSON, and return its
/// length in bytes; `pond_snapshot_ptr` says where it is.
#[unsafe(no_mangle)]
pub extern "C" fn pond_snapshot() -> u32 {
    with_pond(|pond| {
        let snap = pond.scene.snapshot();
        let json = &mut pond.snapshot;
        json.clear();
        // names, activities and calls are all plain ASCII, with nothing to escape
        let _ = write!(json, r#"{{"frame":{},"ducks":["#, snap.frame);
        for (k, d) in snap.ducks.iter().enumerate() {
            let kind = match d.kind {
                Kind::Hen => "hen",
                Kind::Drake => "drake",
                Kind::Duckling => "duckling",
            };
            let call = d.call.map_or("null".to_string(), |c| format!("\"{c}\""));
            let _ = write!(
                json,
                r#"{}{{"name":"{}","kind":"{kind}","x":{:.3},"y":{:.3},"activity":"{}","call":{call}}}"#,
                if k > 0 { "," } else { "" },
                d.name,
                d.pos.x,
                d.pos.y,
                d.activity,
            );
        }
        json.push_str(r#"],"peas":["#);
        for (k, p) in snap.peas.iter().enumerate() {
            let _ = write!(json, "{}[{:.3},{:.3}]", if k > 0 { "," } else { "" }, p.x, p.y);
        }
        json.push_str("]}");
        json.len() as u32
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pond_snapshot_ptr() -> *const u8 {
    with_pond(|pond| pond.snapshot.as_ptr())
}
