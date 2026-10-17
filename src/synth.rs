//! Duck sound synthesizer, ported from `duck.py`.
//!
//! Every sound is built from scratch with a source-filter model of a duck's
//! syrinx (a train of air pulses) and vocal tract (a chain of formant
//! resonances). See `duck.py` for notes on how the parameters were tuned.
//!
//! Unlike the Python version, the sample rate is a runtime value so the pond
//! can be rendered at whatever rate the audio device runs at. The water and
//! the pond itself live in `scene.rs`, where they run as a live simulation.

use std::f64::consts::PI;

use num_complex::Complex64;
use rand::rngs::StdRng;
use rand::Rng;
use rand_distr::StandardNormal;

pub fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn normalize(x: &mut [f64]) {
    let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        x.iter_mut().for_each(|v| *v /= peak);
    }
}

// --- filters ---------------------------------------------------------------

/// One second-order section in transposed direct form II, as used by
/// scipy's `lfilter` and `sosfilt`. The state survives coefficient changes.
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2], // a1, a2 with a0 normalised to 1
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        let mut q = Biquad::default();
        q.set(b, a);
        q
    }

    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        self.b = [b[0] / a[0], b[1] / a[0], b[2] / a[0]];
        self.a = [a[1] / a[0], a[2] / a[0]];
    }

    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

pub enum Band {
    Low(f64),
    High(f64),
    Pass(f64, f64),
}

/// Digital Butterworth filter as second-order sections, designed the same
/// way as `scipy.signal.butter`: analog prototype, band transform of the
/// pre-warped edges, then the bilinear transform.
pub fn butter(order: usize, band: Band, sr: f64) -> Vec<Biquad> {
    assert!(order % 2 == 0, "only even orders are supported");
    let fs2 = 2.0 * sr;
    let warp = |f: f64| {
        assert!(f < sr / 2.0, "{f} Hz is above Nyquist at {sr} Hz");
        fs2 * (PI * f / sr).tan()
    };
    let n = order as f64;
    let proto: Vec<Complex64> = (0..order)
        .map(|k| -Complex64::from_polar(1.0, PI * (2.0 * k as f64 - n + 1.0) / (2.0 * n)))
        .collect();

    // analog zeros, poles and gain
    let (zeros, poles, gain): (Vec<Complex64>, Vec<Complex64>, f64) = match band {
        Band::Low(f) => {
            let wo = warp(f);
            (vec![], proto.iter().map(|p| p * wo).collect(), wo.powi(order as i32))
        }
        Band::High(f) => {
            let wo = warp(f);
            let k = 1.0 / proto.iter().map(|p| -p).product::<Complex64>().re;
            (vec![Complex64::new(0.0, 0.0); order], proto.iter().map(|p| wo / p).collect(), k)
        }
        Band::Pass(lo, hi) => {
            let (lo, hi) = (warp(lo), warp(hi));
            let (bw, wo) = (hi - lo, (lo * hi).sqrt());
            let poles = proto
                .iter()
                .flat_map(|p| {
                    let p = p * bw / 2.0;
                    let r = (p * p - wo * wo).sqrt();
                    [p + r, p - r]
                })
                .collect();
            (vec![Complex64::new(0.0, 0.0); order], poles, bw.powi(order as i32))
        }
    };

    // bilinear transform; zeros at infinity land on Nyquist
    let bilinear = |s: &Complex64| (fs2 + s) / (fs2 - s);
    let mut zd: Vec<f64> = zeros.iter().map(|z| bilinear(z).re).collect();
    zd.resize(poles.len(), -1.0);
    let num: Complex64 = zeros.iter().map(|z| fs2 - z).product();
    let den: Complex64 = poles.iter().map(|p| fs2 - p).product();
    let mut k = gain * (num / den).re;

    // one conjugate pole pair and two real zeros per section
    zd.sort_by(f64::total_cmp);
    let upper: Vec<Complex64> = poles.iter().map(bilinear).filter(|p| p.im > 0.0).collect();
    assert_eq!(upper.len() * 2, poles.len());
    upper
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (z1, z2) = (zd[i], zd[zd.len() - 1 - i]);
            let b = [k, -k * (z1 + z2), k * z1 * z2];
            k = 1.0;
            Biquad::new(b, [1.0, -2.0 * p.re, p.norm_sqr()])
        })
        .collect()
}

fn sosfilt(sections: Vec<Biquad>, x: &[f64]) -> Vec<f64> {
    let mut y = x.to_vec();
    for mut s in sections {
        y.iter_mut().for_each(|v| *v = s.tick(*v));
    }
    y
}

/// One formant: centre frequency (per sample), bandwidth Hz, boost dB.
struct Formant(Vec<f64>, f64, f64);

/// Vocal tract as a cascade of resonant peaks.
///
/// Each peak is a peaking-EQ biquad whose poles have the requested
/// bandwidth, so the boosts simply add up in dB and never cancel each other.
/// The centre frequency is updated every 64 samples.
fn formants(sr: f64, x: &[f64], specs: &[Formant]) -> Vec<f64> {
    const BLOCK: usize = 64;
    let mut y = x.to_vec();
    for Formant(freq, bw, boost_db) in specs {
        let amp2 = 10f64.powf(boost_db / 20.0);
        let mut q = Biquad::default();
        for (s, chunk) in y.chunks_mut(BLOCK).enumerate() {
            let f = freq[s * BLOCK];
            let w0 = 2.0 * PI * f / sr;
            let alpha = w0.sin() * bw / (2.0 * f); // pole Q = f / bw
            let mid = -2.0 * w0.cos();
            q.set([1.0 + alpha * amp2, mid, 1.0 - alpha * amp2], [1.0 + alpha, mid, 1.0 - alpha]);
            chunk.iter_mut().for_each(|v| *v = q.tick(*v));
        }
    }
    y
}

// --- mixing ----------------------------------------------------------------

/// Mix `sound` into `track` starting at time `at` (seconds).
fn place<T: Copy>(sr: f64, track: &mut [T], sound: &[T], at: f64, add: impl Fn(&mut T, T)) {
    let i = ((at * sr) as usize).min(track.len());
    for (dst, &src) in track[i..].iter_mut().zip(sound) {
        add(dst, src);
    }
}

// --- synthesizer -----------------------------------------------------------

pub struct Synth {
    sr: f64,
    rng: StdRng,
}

impl Synth {
    pub fn new(sr: f64, rng: StdRng) -> Self {
        Synth { sr, rng }
    }

    fn samples(&self, seconds: f64) -> usize {
        (seconds * self.sr) as usize
    }

    fn times(&self, n: usize) -> Vec<f64> {
        (0..n).map(|i| i as f64 / self.sr).collect()
    }

    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        self.rng.random_range(lo..hi)
    }

    fn normal(&mut self) -> f64 {
        self.rng.sample(StandardNormal)
    }

    fn noise(&mut self, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.normal()).collect()
    }

    /// Smooth random curve of roughly unit amplitude, changing `rate` times per second.
    fn wobble(&mut self, n: usize, rate: f64) -> Vec<f64> {
        let knots = self.noise((n as f64 / self.sr * rate) as usize + 3);
        (0..n)
            .map(|k| {
                let pos = k as f64 / self.sr * rate;
                let i = pos as usize;
                let w = (1.0 - (PI * (pos - i as f64)).cos()) / 2.0;
                knots[i] * (1.0 - w) + knots[i + 1] * w
            })
            .collect()
    }

    fn highpass(&self, x: &[f64], cutoff: f64, order: usize) -> Vec<f64> {
        sosfilt(butter(order, Band::High(cutoff), self.sr), x)
    }

    fn lowpass(&self, x: &[f64], cutoff: f64) -> Vec<f64> {
        sosfilt(butter(2, Band::Low(cutoff), self.sr), x)
    }

    fn bandpass(&self, x: &[f64], lo: f64, hi: f64) -> Vec<f64> {
        sosfilt(butter(2, Band::Pass(lo, hi), self.sr), x)
    }

    fn place(&self, track: &mut [f64], sound: &[f64], at: f64, gain: f64) {
        place(self.sr, track, sound, at, |d, v| *d += v * gain);
    }

    /// Train of short air pulses, one per vibration of the syrinx.
    ///
    /// `doubling` (0..1) makes every other pulse weaker and slightly early,
    /// which is what gives a quack its rasp: a second harmonic series at half
    /// the pitch. Narrower pulses (`width`, seconds) sound brighter.
    fn pulses(
        &mut self,
        f0: &[f64],
        amp: &[f64],
        jitter: f64,
        shimmer: f64,
        doubling: &[f64],
        width: &[f64],
    ) -> Vec<f64> {
        let n = f0.len();
        let sr = self.sr;
        let half = (5.0 * width.iter().cloned().fold(0.0, f64::max) * sr) as usize + 1;
        let mut out = vec![0.0; n + 2 * half + 1];
        let (mut t, mut k) = (0.0f64, 0usize);
        loop {
            let i = (t * sr) as usize;
            if i >= n {
                break;
            }
            let d = doubling[i];
            let mut a = amp[i] * (1.0 + shimmer * self.normal());
            if k % 2 == 1 {
                a *= 1.0 - d;
            }
            // Gaussian-derivative pulse placed at a fractional sample position
            let c = t * sr;
            let start = c as usize;
            for (j, v) in out[start..start + 2 * half + 1].iter_mut().enumerate() {
                let tau = (start as f64 + j as f64 - half as f64 - c) / (width[i] * sr);
                *v += a * -tau * (-0.5 * tau * tau).exp();
            }
            let period = 1.0 / f0[i] * (1.0 + jitter * self.normal());
            t += period * (1.0 + 0.12 * d * if k % 2 == 1 { 1.0 } else { -1.0 });
            k += 1;
        }
        out[half..half + n].to_vec()
    }

    // --- single calls ------------------------------------------------------

    /// One female-duck quack, peak-normalised.
    pub fn quack(&mut self, dur: f64, f0: f64, f2: f64, rasp: f64) -> Vec<f64> {
        let (f1, breath) = (780.0, 0.05);
        let n = self.samples(dur + 0.03);
        let t = self.times(n);
        let u: Vec<f64> = t.iter().map(|t| (t / dur).clamp(0.0, 1.0)).collect();
        // ~40 ms swell, a slight sag, then a ~35 ms fade
        let w = self.wobble(n, 40.0);
        let env: Vec<f64> = (0..n)
            .map(|i| {
                smoothstep(t[i] / 0.065)
                    * (1.0 - 0.15 * u[i])
                    * (1.0 - smoothstep((t[i] - dur + 0.055) / 0.055))
                    * (1.0 + 0.15 * w[i])
            })
            .collect();
        // pitch slides up into the note, sags slowly, and drops at the very end
        let w = self.wobble(n, 15.0);
        let pitch: Vec<f64> = (0..n)
            .map(|i| {
                f0 * (1.0 - 0.12 * (-t[i] / 0.025).exp())
                    * (1.0 - 0.04 * u[i])
                    * (1.0 - 0.09 * smoothstep((u[i] - 0.75) / 0.25))
                    * (1.0 + 0.012 * w[i])
            })
            .collect();
        let w = self.wobble(n, 8.0);
        let depth: Vec<f64> = w.iter().map(|w| (rasp * (0.8 + 0.3 * w)).clamp(0.0, 0.95)).collect();
        let mut src = self.pulses(&pitch, &env, 0.02, 0.1, &depth, &vec![90e-6; n]);
        let noise = self.noise(n);
        let hiss = self.highpass(&noise, 1000.0, 2);
        for i in 0..n {
            src[i] += breath * env[i] * hiss[i];
        }
        // the bill opens at the start ("qu-") and starts closing at the end ("-ck")
        let shape: Vec<f64> = t
            .iter()
            .map(|t| smoothstep(t / 0.04) - 0.5 * smoothstep((t - dur + 0.04) / 0.04))
            .collect();
        let track = |base: f64, lo: f64| shape.iter().map(|s| base * (lo + (1.0 - lo) * s)).collect();
        let y = formants(self.sr, &src, &[
            Formant(track(f1, 0.88), 150.0, 17.0),
            Formant(track(1500.0, 0.95), 700.0, 10.0),
            Formant(track(f2, 0.93), 280.0, 22.0),
            Formant(vec![3400.0; n], 600.0, 9.0),
        ]);
        let mut y = self.lowpass(&self.highpass(&y, 400.0, 2), 6000.0);
        normalize(&mut y);
        y
    }

    /// The drake's soft, raspy "rhaeb": irregular pulses that speed up and swell.
    pub fn rhaeb(&mut self, dur: f64, f1: f64) -> Vec<f64> {
        let rate = (88.0, 108.0);
        let n = self.samples(dur + 0.03);
        let t = self.times(n);
        let u: Vec<f64> = t.iter().map(|t| (t / dur).clamp(0.0, 1.0)).collect();
        let env: Vec<f64> = (0..n)
            .map(|i| {
                (0.45 + 0.55 * smoothstep(u[i] / 0.85))
                    * smoothstep(t[i] / 0.015)
                    * (1.0 - smoothstep((t[i] - dur + 0.045) / 0.045))
            })
            .collect();
        let w = self.wobble(n, 20.0);
        let pps: Vec<f64> =
            (0..n).map(|i| (rate.0 + (rate.1 - rate.0) * u[i]) * (1.0 + 0.03 * w[i])).collect();
        // pulses sharpen as the call gets louder, so the end is brighter
        let width: Vec<f64> = u.iter().map(|u| 130e-6 - 50e-6 * u).collect();
        let mut src = self.pulses(&pps, &env, 0.12, 0.3, &vec![0.0; n], &width);
        // each pulse trails off into hiss, and there is a little hiss throughout
        let noise = self.noise(n);
        let hiss = self.highpass(&noise, 800.0, 2);
        let mut phase = 0.0;
        for i in 0..n {
            phase += pps[i] / self.sr;
            let gate = (-3.0 * phase.rem_euclid(1.0)).exp() + 0.3;
            src[i] += 0.12 * env[i] * gate * hiss[i];
        }
        let opening: Vec<f64> = u.iter().map(|u| 0.9 + 0.12 * smoothstep(u / 0.9)).collect();
        let track = |base: f64| opening.iter().map(|o| base * o).collect();
        let y = formants(self.sr, &src, &[
            Formant(track(0.93 * f1), 260.0, 17.0),
            Formant(track(1.12 * f1), 280.0, 16.0),
            Formant(track(2880.0), 350.0, 9.0),
            Formant(vec![4400.0; n], 600.0, 5.0),
        ]);
        let mut y = self.lowpass(&self.highpass(&y, 900.0, 4), 5500.0);
        normalize(&mut y);
        y
    }

    /// One duckling peep: a whistle that rises briefly, holds, then sweeps
    /// down by `sweep` (or up, if it is negative).
    pub fn peep(&mut self, dur: f64, top: f64, sweep: f64) -> Vec<f64> {
        let n = self.samples(dur);
        let t = self.times(n);
        let w = self.wobble(n, 30.0);
        let w_amp = self.wobble(n, 50.0);
        let mut phase = 0.0;
        let mut y: Vec<f64> = (0..n)
            .map(|i| {
                let u = t[i] / dur;
                let freq = top
                    * (0.88 + 0.12 * smoothstep(u / 0.18))
                    * (1.0 - sweep * smoothstep((u - 0.45) / 0.55))
                    * (1.0 + 0.01 * w[i]);
                phase += 2.0 * PI * freq / self.sr;
                let tone = phase.sin() + 0.12 * (2.0 * phase + 0.3).sin() + 0.03 * (3.0 * phase + 1.1).sin();
                let env = smoothstep(t[i] / 0.012) * (1.0 - smoothstep((u - 0.6) / 0.4)).powf(1.5);
                tone * env * (1.0 + 0.05 * w_amp[i])
            })
            .collect();
        normalize(&mut y);
        y
    }

    /// A small bubble or drip: a short ringing blip that rises in pitch.
    pub fn bubble(&mut self) -> Vec<f64> {
        let dur = self.uniform(0.012, 0.04);
        let n = self.samples(dur);
        let base = self.uniform(600.0, 2400.0);
        let rise = self.uniform(0.2, 0.6);
        let mut phase = 0.0;
        (0..n)
            .map(|i| {
                let t = i as f64 / self.sr;
                phase += 2.0 * PI * base * (1.0 + rise * t / dur) / self.sr;
                phase.sin() * (-4.0 * t / dur).exp() * smoothstep(t / 0.0007)
            })
            .collect()
    }

    // --- sequences ---------------------------------------------------------

    /// Evenly spaced full quacks: "quack quack quack".
    pub fn series(&mut self, count: usize, f0: f64, f2: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(0.36 * count as f64 + 0.15)];
        for k in 0..count {
            let k = k as f64;
            let note = self.quack(0.23 - 0.01 * k, f0, f2, 0.5);
            self.place(&mut track, &note, 0.05 + 0.36 * k, 1.0 - 0.03 * k);
        }
        track
    }

    /// The hen's classic call: QUACK QUACK QUACK quack quack quack quack.
    pub fn decrescendo(&mut self, notes: usize, f0: f64, f2: f64) -> Vec<f64> {
        const DURS: [f64; 7] = [0.235, 0.225, 0.21, 0.175, 0.155, 0.14, 0.125];
        const GAINS: [f64; 7] = [0.9, 1.0, 1.0, 0.85, 0.68, 0.56, 0.45];
        let notes = notes.min(DURS.len());
        let mut track = vec![0.0; self.samples(0.36 * notes as f64 + 0.3)];
        let mut at = 0.05;
        for (k, (dur, gain)) in DURS.iter().zip(GAINS).take(notes).enumerate() {
            let k = k as f64;
            let dur = dur + self.uniform(-0.005, 0.005);
            let note = self.quack(dur, f0 * (1.0 - 0.008 * k), f2 * (1.0 - 0.015 * k), 0.55 - 0.02 * k);
            self.place(&mut track, &note, at, gain);
            at += 0.35 + 0.006 * k + self.uniform(-0.008, 0.008);
        }
        track
    }

    /// Quieter, quicker conversational quacks while feeding: "wak-wak-wak ... wak".
    pub fn chatter(&mut self, seconds: f64, f0: f64, f2: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(seconds + 0.2)];
        let mut at = 0.05;
        while at < seconds - 0.1 {
            for _ in 0..self.rng.random_range(2..6) {
                if at > seconds - 0.1 {
                    break;
                }
                let dur = self.uniform(0.12, 0.17);
                let (pf, pf2, rasp) =
                    (self.uniform(0.94, 1.06), self.uniform(0.95, 1.03), self.uniform(0.3, 0.7));
                let note = self.quack(dur, f0 * pf, f2 * pf2, rasp);
                let gain = self.uniform(0.35, 0.6);
                self.place(&mut track, &note, at, gain);
                at += dur + self.uniform(0.08, 0.15);
            }
            at += self.uniform(0.2, 0.5);
        }
        track
    }

    /// A drake muttering "rhaeb ... rhaeb ... rhaeb" while he potters about.
    pub fn drake(&mut self, calls: usize, gap: f64, f1: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(gap * calls as f64 + 0.2)];
        for k in 0..calls {
            let dur = self.uniform(0.27, 0.33);
            let call = self.rhaeb(dur, f1);
            let at = 0.05 + k as f64 * gap + self.uniform(-0.05, 0.05);
            self.place(&mut track, &call, at, 1.0);
        }
        track
    }

    /// A duckling calling for its mother: peeps about every 0.27 s, in bursts.
    pub fn peeping(&mut self, seconds: f64, top: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(seconds + 0.2)];
        let mut at = 0.05;
        while at < seconds - 0.2 {
            for _ in 0..self.rng.random_range(4..9) {
                if at > seconds - 0.15 {
                    break;
                }
                let (dur, pitch) = (self.uniform(0.11, 0.15), self.uniform(0.96, 1.04));
                let note = self.peep(dur, top * pitch, 0.35);
                let gain = self.uniform(0.7, 1.0);
                self.place(&mut track, &note, at, gain);
                at += 0.267 * self.uniform(0.9, 1.1);
            }
            at += self.uniform(0.3, 0.8);
        }
        track
    }

    /// A content duckling keeping in touch with its brood: a few soft notes,
    /// shorter, lower and quicker than its distress peeps, and rising rather
    /// than falling.
    pub fn contact(&mut self, notes: usize, top: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(0.13 * notes as f64 + 0.1)];
        let mut at = 0.02;
        for _ in 0..notes {
            let (dur, pitch) = (self.uniform(0.045, 0.065), self.uniform(0.96, 1.04));
            let note = self.peep(dur, 0.85 * top * pitch, -0.12);
            let gain = self.uniform(0.6, 1.0);
            self.place(&mut track, &note, at, gain);
            at += self.uniform(0.1, 0.13);
        }
        track
    }

    /// A hen gathering her brood: short, soft "kwek"s at about 3.7 a second,
    /// the pace her ducklings know her call by.
    pub fn brood_call(&mut self, notes: usize, f0: f64, f2: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(0.27 * notes as f64 + 0.2)];
        let mut at = 0.05;
        for _ in 0..notes {
            let dur = self.uniform(0.08, 0.1);
            let (pf, pf2, rasp) = (self.uniform(0.97, 1.03), self.uniform(0.96, 1.0), self.uniform(0.3, 0.5));
            let note = self.quack(dur, f0 * pf, f2 * pf2, rasp);
            let gain = self.uniform(0.8, 1.0);
            self.place(&mut track, &note, at, gain);
            at += 0.27 * self.uniform(0.95, 1.05);
        }
        track
    }

    /// A bill sieving the water surface: a quick patter of tiny splashes.
    pub fn dabbling(&mut self, seconds: f64) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(seconds + 0.1)];
        let mut at = 0.0;
        while at < seconds {
            let len = self.uniform(0.004, 0.02);
            let splash = self.splash(len);
            let gain = self.uniform(0.3, 1.0);
            self.place(&mut track, &splash, at, gain);
            if self.rng.random::<f64>() < 0.15 {
                let b = self.bubble();
                let gain = self.uniform(0.2, 0.5);
                self.place(&mut track, &b, at, gain);
            }
            at += self.uniform(0.03, 0.12);
        }
        normalize(&mut track);
        track
    }

    /// One tiny splash of the bill at the water surface.
    fn splash(&mut self, seconds: f64) -> Vec<f64> {
        let n = self.samples(seconds);
        let noise = self.noise(n);
        let mut splash = self.bandpass(&noise, 900.0, 6000.0);
        for (i, v) in splash.iter_mut().enumerate() {
            *v *= (-(i as f64) / (0.25 * n as f64)).exp();
        }
        splash
    }

    /// The bill clacking shut: a tick ringing at the hollow of the bill.
    fn clack(&mut self, ring: f64) -> Vec<f64> {
        let n = self.samples(0.012);
        let noise = self.noise(n);
        let src: Vec<f64> = (0..n).map(|i| noise[i] * (-(i as f64) / self.sr / 0.0006).exp()).collect();
        let y = formants(self.sr, &src, &[
            Formant(vec![ring; n], 400.0, 18.0),
            Formant(vec![1.7 * ring; n], 600.0, 10.0),
        ]);
        self.highpass(&y, 700.0, 2)
    }

    /// A soft swallow: a short low blip that drops in pitch.
    fn gulp(&mut self) -> Vec<f64> {
        let n = self.samples(0.06);
        let (top, bottom) = (self.uniform(450.0, 600.0), self.uniform(260.0, 320.0));
        let mut phase = 0.0;
        (0..n)
            .map(|i| {
                let t = i as f64 / self.sr;
                phase += 2.0 * PI * (bottom + (top - bottom) * (-t / 0.012).exp()) / self.sr;
                (phase.sin() + 0.2 * (2.0 * phase).sin()) * smoothstep(t / 0.004) * (-t / 0.015).exp()
            })
            .collect()
    }

    /// A duck snapping up a pea: a flurry of bill clacks, each with a splash
    /// when it is on the water, then a gulp. Peak-normalised.
    pub fn peck(&mut self, on_water: bool) -> Vec<f64> {
        let mut track = vec![0.0; self.samples(0.45)];
        let ring = self.uniform(2200.0, 3200.0);
        let mut at = 0.01;
        for _ in 0..self.rng.random_range(2..=4) {
            let pitch = ring * self.uniform(0.95, 1.05);
            let clack = self.clack(pitch);
            let gain = self.uniform(0.5, 1.0);
            self.place(&mut track, &clack, at, gain);
            if on_water {
                let len = self.uniform(0.006, 0.018);
                let splash = self.splash(len);
                let gain = self.uniform(0.3, 0.7);
                self.place(&mut track, &splash, at + 0.002, gain);
            }
            at += self.uniform(0.03, 0.06);
        }
        let gulp = self.gulp();
        let at = at + self.uniform(0.04, 0.09);
        self.place(&mut track, &gulp, at, 0.5);
        normalize(&mut track);
        track
    }

    /// A pea landing on the bank: a tiny dull tap. Peak-normalised.
    pub fn tap(&mut self) -> Vec<f64> {
        let n = self.samples(0.006);
        let noise = self.noise(n);
        let src: Vec<f64> = (0..n).map(|i| noise[i] * (-(i as f64) / self.sr / 0.0008).exp()).collect();
        let mut y = self.bandpass(&src, 1000.0, 5000.0);
        normalize(&mut y);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(sections: &[Biquad], f: f64, sr: f64) -> f64 {
        let z = Complex64::from_polar(1.0, 2.0 * PI * f / sr);
        sections
            .iter()
            .map(|s| {
                let num = s.b[0] + s.b[1] / z + s.b[2] / (z * z);
                let den = 1.0 + s.a[0] / z + s.a[1] / (z * z);
                (num / den).norm()
            })
            .product()
    }

    #[test]
    fn butterworth_edges_are_minus_3db() {
        let sr = 44100.0;
        let half_power = 0.5f64.sqrt();
        let lp = butter(2, Band::Low(6000.0), sr);
        assert!((gain_at(&lp, 6000.0, sr) - half_power).abs() < 1e-9);
        assert!((gain_at(&lp, 1.0, sr) - 1.0).abs() < 1e-6);
        let hp = butter(4, Band::High(900.0), sr);
        assert!((gain_at(&hp, 900.0, sr) - half_power).abs() < 1e-9);
        assert!((gain_at(&hp, sr / 2.0 - 1.0, sr) - 1.0).abs() < 1e-6);
        let bp = butter(2, Band::Pass(90.0, 700.0), sr);
        assert!((gain_at(&bp, 90.0, sr) - half_power).abs() < 1e-9);
        assert!((gain_at(&bp, 700.0, sr) - half_power).abs() < 1e-9);
        assert!((gain_at(&bp, (90.0f64 * 700.0).sqrt(), sr) - 1.0).abs() < 1e-3);
    }
}
