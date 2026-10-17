//! A live duck pond: ducks with positions and simple behaviour, heard from a
//! fixed listening spot on the bank.
//!
//! The listener stands at the origin facing north (+y) across the water,
//! with distances in metres. Ducks spend their time much as mallards do:
//! minutes on end dabbling, resting and preening, with now and then a swim
//! somewhere else, and they mostly keep quiet unless something is going on.
//! A drake keeps by his mate and answers her. Ducklings stay close to their
//! mother and twitter softly; one that wanders off and loses her peeps until
//! she calls it back. Every sound is rendered from the duck's position at
//! that moment: panned by bearing (or on headphones, reaching each ear around
//! a model head), quieter and duller with distance, delayed by the speed of
//! sound and mixed into a shared reverb, so far ducks sound far away.

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI, TAU};
use std::ops::{Add, Mul, Sub};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::StandardNormal;

use crate::synth::{Band, Biquad, Synth, butter, smoothstep};

// --- geometry --------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }

    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }

    fn unit(self) -> Vec2 {
        let l = self.len();
        if l > 1e-9 { self * (1.0 / l) } else { Vec2::default() }
    }

    fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

/// The pond is an ellipse whose near shore is 3 m in front of the listener
/// and whose far shore is 45 m away.
pub const POND_CENTER: Vec2 = Vec2::new(0.0, 24.0);
pub const POND_RADII: Vec2 = Vec2::new(34.0, 21.0);
/// Strip of bank around the listener where ducks may come ashore.
pub const BANK_MIN: Vec2 = Vec2::new(-9.0, -3.0);
pub const BANK_MAX: Vec2 = Vec2::new(9.0, 4.5);
/// Ducks keep at least this far from the listener.
const PERSONAL_SPACE: f64 = 1.0;

pub fn in_pond(p: Vec2) -> bool {
    let d = p - POND_CENTER;
    (d.x / POND_RADII.x).powi(2) + (d.y / POND_RADII.y).powi(2) <= 1.0
}

pub fn on_bank(p: Vec2) -> bool {
    (BANK_MIN.x..=BANK_MAX.x).contains(&p.x) && (BANK_MIN.y..=BANK_MAX.y).contains(&p.y)
}

fn allowed(p: Vec2) -> bool {
    (in_pond(p) || on_bank(p)) && p.len() > PERSONAL_SPACE
}

/// Preferred distances from the listener. Ducks drift between these over
/// time, so there is always a mix of close and far birds.
const NEAR: (f64, f64) = (3.0, 10.0);
const MID: (f64, f64) = (10.0, 25.0);
const FAR: (f64, f64) = (25.0, 46.0);

fn pond_point(rng: &mut StdRng, (lo, hi): (f64, f64)) -> Vec2 {
    let mut any = POND_CENTER;
    for _ in 0..500 {
        let p = POND_CENTER
            + Vec2::new(rng.random_range(-1.0..1.0) * POND_RADII.x, rng.random_range(-1.0..1.0) * POND_RADII.y);
        if in_pond(p) && allowed(p) {
            any = p;
            if (lo..hi).contains(&p.len()) {
                return p;
            }
        }
    }
    any
}

/// A spot on the bank at least `away` from the listener.
fn bank_point(rng: &mut StdRng, away: f64) -> Vec2 {
    loop {
        let p = Vec2::new(rng.random_range(BANK_MIN.x..BANK_MAX.x), rng.random_range(BANK_MIN.y..2.5));
        if p.len() > away {
            return p;
        }
    }
}

/// As far as a duck at `from` can get towards `to` without leaving the
/// water or the bank.
fn clamp_towards(from: Vec2, to: Vec2) -> Vec2 {
    if allowed(to) {
        return to;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        if allowed(from + (to - from) * mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    from + (to - from) * lo
}

// --- ducks -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Hen,
    Drake,
    Duckling,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Activity {
    /// Going somewhere: swimming, or waddling ashore.
    Swim,
    /// Feeding: sieving the surface and upending, or grazing ashore. For
    /// ducklings, picking insects off the water.
    Dabble,
    /// Loafing, or asleep.
    Rest,
    Preen,
    /// A duckling that has wandered off after insects.
    Stray,
    /// Going from pea to pea until they are all gone.
    Feed,
}

/// Paddling ducks can't outswim the waves they make: hull speed is 0.71 m/s
/// for an adult and 0.43 m/s for a 9-day-old duckling, which paddles at
/// 0.33 m/s on average and bursts across the surface at 1.26 m/s to catch up
/// (Prange & Schmidt-Nielsen, 1970; Aigeldinger & Fish, 1995).
const HULL_SPEED: f64 = 0.71;
const DUCKLING_HULL_SPEED: f64 = 0.43;
const DUCKLING_BURST: f64 = 1.26;
/// A hen leading ducklings goes at a pace they can paddle.
const BROOD_PACE: f64 = 0.3;
/// A duckling further than this from its mother calls for her.
const LOST: f64 = 4.0;
/// Mean time between a foraging duckling's wanders away from its mother.
const STRAY_EVERY: f64 = 900.0;

/// A pea thrown by the listener: in the air for a moment, then floating or
/// lying on the bank until a duck eats it.
struct Pea {
    pos: Vec2,
    airborne: f64,
    age: f64,
}

/// Uneaten peas sink (or get lost in the grass) after this long.
const PEA_LIFETIME: f64 = 180.0;
const MAX_PEAS: usize = 120;

struct Duck {
    name: &'static str,
    kind: Kind,
    /// Hen: (f0, f2) of her quack. Drake: (f1, -) of his rhaeb. Duckling: (peep top, -).
    voice: (f64, f64),
    pos: Vec2,
    vel: Vec2,
    facing: Vec2,
    target: Vec2,
    cruise: f64,
    activity: Activity,
    activity_left: f64,
    /// Who it keeps with, doing much as they do: ducklings their mother, a
    /// drake his mate.
    leader: Option<usize>,
    /// Place in the line behind the mother, for ducklings.
    rank: usize,
    /// Where to keep relative to the leader (x right, y ahead), re-picked now
    /// and then. Ducks on their own potter about their feeding spot instead.
    offset: Vec2,
    offset_left: f64,
    band: (f64, f64),
    call_timer: f64,
    call: Option<(&'static str, f64)>,
    /// Calling back to another duck, which sets no one else off.
    answering: bool,
    /// A hen about to call her ducklings together.
    gathering: bool,
    /// A duckling too far from its mother.
    lost: bool,
    /// Countdown to the next spell of sieving the surface, when dabbling.
    splash_timer: f64,
    /// Countdown until the duck notices freshly thrown peas.
    notice_peas: Option<f64>,
    /// Time left chewing the last pea.
    eating: f64,
}

impl Duck {
    fn new(name: &'static str, kind: Kind, voice: (f64, f64), pos: Vec2, rng: &mut StdRng) -> Self {
        Duck {
            name,
            kind,
            voice,
            pos,
            vel: Vec2::default(),
            facing: Vec2::new(0.0, 1.0),
            target: pos,
            cruise: match kind {
                Kind::Hen => 0.45,
                Kind::Drake => 0.5,
                Kind::Duckling => 0.33,
            },
            activity: Activity::Rest,
            activity_left: rng.random_range(0.0..4.0),
            leader: None,
            rank: 0,
            offset: Vec2::default(),
            offset_left: 0.0,
            band: MID,
            call_timer: rng.random_range(0.5..8.0),
            call: None,
            answering: false,
            gathering: false,
            lost: false,
            splash_timer: 0.0,
            notice_peas: None,
            eating: 0.0,
        }
    }

    fn activity_label(&self) -> &'static str {
        let ashore = !in_pond(self.pos);
        let duckling = self.kind == Kind::Duckling;
        match self.activity {
            _ if self.lost => "calling for mum",
            Activity::Swim if duckling => "following mum",
            Activity::Swim if self.leader.is_some() => "escorting",
            Activity::Swim if ashore => "waddling",
            Activity::Swim => "swimming",
            Activity::Dabble if duckling => "picking insects",
            Activity::Dabble if ashore => "grazing",
            Activity::Dabble => "dabbling",
            Activity::Rest if duckling => "huddled by mum",
            Activity::Rest => "resting",
            Activity::Preen => "preening",
            Activity::Stray => "wandering off",
            Activity::Feed if self.eating > 0.0 => "eating a pea",
            Activity::Feed => "after the peas",
        }
    }
}

/// Where a duck is shown and heard, as seen by the visualizer.
#[derive(Clone, Debug)]
pub struct DuckView {
    pub name: &'static str,
    pub kind: Kind,
    pub pos: Vec2,
    pub activity: &'static str,
    pub call: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Output frame at which this state starts being heard.
    pub frame: u64,
    pub ducks: Vec<DuckView>,
    /// Peas that have landed and not been eaten yet.
    pub peas: Vec<Vec2>,
}

/// Something the listener does to the pond.
pub enum Command {
    ScatterPeas,
}

// --- spatial audio ---------------------------------------------------------

/// Distance within which sounds play at their full level.
const REF_DISTANCE: f64 = 2.0;
const SPEED_OF_SOUND: f64 = 343.0;
/// Air absorption at 4 kHz in dB per metre (ISO 9613-1, 20 °C and 70 % humidity).
const AIR_ABSORPTION: f64 = 0.0231;
/// Sounds from straight behind are lowpassed here, shaded by the head.
const REAR_CUTOFF: f64 = 10000.0;
/// Radius of the listener's head, for headphones.
const HEAD_RADIUS: f64 = 0.0875;
/// Reverb level relative to the direct sound at `REF_DISTANCE`. The reverb
/// falls off more slowly than the direct sound, so distant ducks are wetter.
const REVERB_SEND: f64 = 0.25;
const REVERB_FALLOFF: f64 = 0.25;
const WATER_LEVEL: f64 = 0.017;
const MASTER: f64 = 0.9;

// call levels at REF_DISTANCE, balanced as in duck.py's pond
const QUACK: f64 = 0.9;
const SERIES: f64 = 0.8;
const DECRESCENDO: f64 = 1.0;
const CHATTER: f64 = 0.55;
const BROOD_CALL: f64 = 0.4;
const RHAEB: f64 = 0.45;
const PEEPS: f64 = 0.15;
const CONTACT: f64 = 0.04;
const DABBLING: f64 = 0.15;
const PECK: f64 = 0.3;
const DUCKLING_PECK: f64 = 0.18;
const PEA_PLOP: f64 = 0.06;
const PEA_TAP: f64 = 0.05;

/// How a sound reaches one ear, or without headphones, one speaker.
#[derive(Clone, Copy)]
struct Ear {
    gain: f64,
    /// Frames after it would reach an ear facing it.
    delay: f64,
    /// High-frequency gain around the head; low frequencies get round unchanged.
    bright: f64,
}

#[derive(Clone, Copy)]
struct Gains {
    /// Left and right.
    ears: [Ear; 2],
    send: f64,
    /// One-pole lowpass coefficient: distant and rear sounds are duller.
    lowpass: f64,
}

fn spatialize(pos: Vec2, level: f64, sr: f64, headphones: bool) -> Gains {
    let r = pos.len().max(REF_DISTANCE);
    let atten = REF_DISTANCE / r;
    // A one-pole lowpass takes about 4.3 (f / cutoff)² dB off below its
    // cutoff, rising with f² much as air absorption does, so it stands in for
    // the air with the cutoff that loses as much at 4 kHz. Losses in dB add,
    // so the head's shading of sounds from behind adds its 1 / cutoff².
    let air = (10f64.powf(AIR_ABSORPTION * r / 10.0) - 1.0) / 4000f64.powi(2);
    let rear = if pos.y < 0.0 { -pos.y / r / REAR_CUTOFF.powi(2) } else { 0.0 };
    let cutoff = (air + rear).sqrt().recip();
    let ears = if headphones {
        // A spherical head (Brown & Duda, 1998), with θ the angle between the
        // sound and the ear's axis. The ear facing the sound hears it first,
        // its highs doubled; the far one up to 0.66 ms later, its highs cut
        // to a tenth at 150° but less right behind, where the waves round
        // either side meet. Both get the level of a centred pan.
        let side = pos.unit().x.clamp(-1.0, 1.0);
        [-side, side].map(|facing| {
            let theta = facing.acos();
            let lag = if theta < FRAC_PI_2 { 1.0 - facing } else { 1.0 + theta - FRAC_PI_2 };
            Ear {
                gain: level * atten * FRAC_1_SQRT_2,
                delay: lag * HEAD_RADIUS / SPEED_OF_SOUND * sr,
                bright: 1.05 + 0.95 * (PI * theta / 150f64.to_radians()).cos(),
            }
        })
    } else {
        // bearing 0 straight ahead, +90° to the right; constant-power pan
        let angle = (pos.x.atan2(pos.y).sin() + 1.0) * FRAC_PI_4;
        [angle.cos(), angle.sin()].map(|pan| Ear { gain: level * atten * pan, delay: 0.0, bright: 1.0 })
    };
    Gains {
        ears,
        send: level * REVERB_SEND * atten.powf(REVERB_FALLOFF),
        lowpass: 1.0 - (-TAU * cutoff / sr).exp(),
    }
}

/// `samples` at a fractional frame position, silent before and after,
/// interpolated through the neighbouring frames (Catmull-Rom).
fn sample_at(samples: &[f64], at: f64) -> f64 {
    if at <= -2.0 || at >= samples.len() as f64 + 1.0 {
        return 0.0;
    }
    let i = at.floor();
    let f = at - i;
    let s = |j: f64| if j < 0.0 { 0.0 } else { samples.get(j as usize).copied().unwrap_or(0.0) };
    let (a, b, c, d) = (s(i - 1.0), s(i), s(i + 1.0), s(i + 2.0));
    b + 0.5 * f * (c - a + f * (2.0 * a - 5.0 * b + 4.0 * c - d + f * (3.0 * (b - c) + d - a)))
}

enum Source {
    Duck(usize),
    Fixed(Vec2),
}

/// A rendered call travelling from its source to the listener.
struct Voice {
    samples: Vec<f64>,
    /// Frame reaching the middle of the head now.
    at: usize,
    delay: usize,
    source: Source,
    level: f64,
    lowpassed: f64,
    ears: [Heard; 2],
    gains: Option<Gains>,
}

/// One ear's filters for a voice: the air's lowpass, then the head's.
#[derive(Clone, Copy, Default)]
struct Heard {
    lowpassed: f64,
    shaded: f64,
}

/// Smooth random curve of roughly unit amplitude, as `Synth::wobble` but streamed.
struct Wobble {
    from: f64,
    to: f64,
    phase: f64,
    step: f64,
}

impl Wobble {
    fn new(rate: f64, sr: f64, rng: &mut StdRng) -> Self {
        Wobble { from: rng.sample(StandardNormal), to: rng.sample(StandardNormal), phase: 0.0, step: rate / sr }
    }

    fn tick(&mut self, rng: &mut StdRng) -> f64 {
        self.phase += self.step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.from = self.to;
            self.to = rng.sample(StandardNormal);
        }
        let w = (1.0 - (PI * self.phase).cos()) / 2.0;
        self.from * (1.0 - w) + self.to * w
    }
}

/// RMS of a filter's output for unit white noise.
fn noise_gain(mut filter: Vec<Biquad>, sr: f64) -> f64 {
    let mut energy = 0.0;
    for i in 0..(2.0 * sr) as usize {
        let mut y = if i == 0 { 1.0 } else { 0.0 };
        for s in filter.iter_mut() {
            y = s.tick(y);
        }
        energy += y * y;
    }
    energy.sqrt()
}

/// Water gently lapping at the bank by the listener, decorrelated per ear.
struct Water {
    filters: [Vec<Biquad>; 2],
    swells: [Wobble; 2],
    norm: f64,
}

impl Water {
    fn new(sr: f64, rng: &mut StdRng) -> Self {
        let filter = butter(2, Band::Pass(90.0, 700.0), sr);
        Water {
            norm: WATER_LEVEL / noise_gain(filter.clone(), sr),
            filters: [filter.clone(), filter],
            swells: [Wobble::new(0.7, sr, rng), Wobble::new(0.7, sr, rng)],
        }
    }

    fn tick(&mut self, rng: &mut StdRng) -> [f64; 2] {
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let mut y: f64 = rng.sample(StandardNormal);
            for s in self.filters[c].iter_mut() {
                y = s.tick(y);
            }
            let swell = 0.35 + 0.65 * smoothstep(0.5 + 0.35 * self.swells[c].tick(rng));
            *o = y * self.norm * swell;
        }
        out
    }
}

#[derive(Clone)]
struct Delay {
    buf: Vec<f64>,
    i: usize,
    damped: f64,
}

impl Delay {
    fn new(len: usize) -> Self {
        Delay { buf: vec![0.0; len.max(1)], i: 0, damped: 0.0 }
    }

    fn comb(&mut self, x: f64, feedback: f64, damp: f64) -> f64 {
        let y = self.buf[self.i];
        self.damped = y * (1.0 - damp) + self.damped * damp;
        self.buf[self.i] = x + self.damped * feedback;
        self.i = (self.i + 1) % self.buf.len();
        y
    }

    fn allpass(&mut self, x: f64) -> f64 {
        let b = self.buf[self.i];
        self.buf[self.i] = x + 0.5 * b;
        self.i = (self.i + 1) % self.buf.len();
        b - x
    }
}

/// A small Freeverb-style reverb: short and fairly dark, like the trees and
/// water around a pond rather than a hall.
#[derive(Clone)]
struct Reverb {
    combs: [Vec<Delay>; 2],
    allpasses: [Vec<Delay>; 2],
    norm: f64,
}

impl Reverb {
    const COMBS: [usize; 4] = [1116, 1277, 1422, 1557];
    const ALLPASSES: [usize; 2] = [556, 341];
    const SPREAD: usize = 23;
    const FEEDBACK: f64 = 0.75;
    const DAMP: f64 = 0.4;

    fn new(sr: f64) -> Self {
        let scale = |n: usize, c: usize| ((n + c * Self::SPREAD) as f64 * sr / 44100.0) as usize;
        let bank = |sizes: &[usize], c| sizes.iter().map(|&n| Delay::new(scale(n, c))).collect();
        let mut reverb = Reverb {
            combs: [bank(&Self::COMBS, 0), bank(&Self::COMBS, 1)],
            allpasses: [bank(&Self::ALLPASSES, 0), bank(&Self::ALLPASSES, 1)],
            norm: 1.0,
        };
        // scale so unit white noise in gives unit RMS out
        let mut probe = reverb.clone();
        let energy: f64 = (0..(3.0 * sr) as usize)
            .map(|i| probe.tick(if i == 0 { 1.0 } else { 0.0 })[0].powi(2))
            .sum();
        reverb.norm = 1.0 / energy.sqrt();
        reverb
    }

    fn tick(&mut self, x: f64) -> [f64; 2] {
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let mut y: f64 = self.combs[c].iter_mut().map(|d| d.comb(x, Self::FEEDBACK, Self::DAMP)).sum();
            for d in self.allpasses[c].iter_mut() {
                y = d.allpass(y);
            }
            *o = y * self.norm;
        }
        out
    }
}

/// Transparent below 0.7, then rounds off smoothly towards 1.
fn soft_limit(x: f64) -> f64 {
    const KNEE: f64 = 0.7;
    if x.abs() <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((x.abs() - KNEE) / (1.0 - KNEE)).tanh())
    }
}

// --- the scene -------------------------------------------------------------

pub struct Scene {
    sr: f64,
    synth: Synth,
    rng: StdRng,
    ducks: Vec<Duck>,
    voices: Vec<Voice>,
    peas: Vec<Pea>,
    water: Water,
    reverb: Reverb,
    bubble_timer: f64,
    frame: u64,
    mix: Vec<[f64; 2]>,
    send: Vec<f64>,
    /// Render for headphones rather than speakers.
    headphones: bool,
}

impl Scene {
    pub fn new(sr: f64, mut rng: StdRng, headphones: bool) -> Self {
        let synth = Synth::new(sr, StdRng::seed_from_u64(rng.random()));
        let water = Water::new(sr, &mut rng);
        let ducks = flock(&mut rng);
        Scene {
            sr,
            headphones,
            synth,
            water,
            reverb: Reverb::new(sr),
            ducks,
            rng,
            voices: Vec::new(),
            peas: Vec::new(),
            bubble_timer: 0.0,
            frame: 0,
            mix: Vec::new(),
            send: Vec::new(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            frame: self.frame,
            ducks: self
                .ducks
                .iter()
                .map(|d| DuckView {
                    name: d.name,
                    kind: d.kind,
                    pos: d.pos,
                    activity: d.activity_label(),
                    call: d.call.map(|(label, _)| label),
                })
                .collect(),
            peas: self.peas.iter().filter(|p| p.airborne <= 0.0).map(|p| p.pos).collect(),
        }
    }

    pub fn handle(&mut self, command: Command) {
        match command {
            Command::ScatterPeas => self.scatter_peas(),
        }
    }

    /// Switch between rendering for headphones and for speakers. Sounds
    /// already on their way glide over to the new placement.
    pub fn set_headphones(&mut self, headphones: bool) {
        self.headphones = headphones;
    }

    /// Throw a handful of peas into the arc in front of the listener. Every
    /// duck notices, the nearest first, and comes over to eat.
    fn scatter_peas(&mut self) {
        let count = self.rng.random_range(12..=20).min(MAX_PEAS.saturating_sub(self.peas.len()));
        for _ in 0..count {
            let pos = loop {
                let bearing = self.rng.random_range(-35f64..35.0).to_radians();
                let p = Vec2::new(bearing.sin(), bearing.cos()) * self.rng.random_range(2.0..7.0);
                if allowed(p) {
                    break p;
                }
            };
            let airborne = 0.25 + 0.07 * pos.len() + self.rng.random_range(0.0..0.2);
            self.peas.push(Pea { pos, airborne, age: 0.0 });
        }
        if count == 0 {
            return;
        }
        let landing = Vec2::new(0.0, 4.5);
        for d in &mut self.ducks {
            let reaction = self.rng.random_range(0.3..1.5) + (d.pos - landing).len() / 12.0;
            if d.activity != Activity::Feed {
                d.notice_peas = Some(d.notice_peas.map_or(reaction, |t| t.min(reaction)));
            }
        }
    }

    fn nearest_pea(&self, pos: Vec2) -> Option<usize> {
        (0..self.peas.len())
            .filter(|&k| self.peas[k].airborne <= 0.0)
            .min_by(|&a, &b| (self.peas[a].pos - pos).len().total_cmp(&(self.peas[b].pos - pos).len()))
    }

    /// Back to normal life once the peas are gone, though ducks on their own
    /// hang about for a while in case more come.
    fn stop_feeding(&mut self, i: usize) {
        self.ducks[i].eating = 0.0;
        if self.ducks[i].leader.is_some() {
            self.cue(i);
            return;
        }
        let r: f64 = self.rng.random();
        let d = &mut self.ducks[i];
        d.activity = if r < 0.5 {
            Activity::Dabble
        } else if r < 0.75 {
            Activity::Rest
        } else {
            Activity::Preen
        };
        d.activity_left = self.rng.random_range(60.0..180.0);
        d.target = d.pos;
        self.follow_lead(i);
    }

    /// Advance the scene by `out.len()` frames and render them.
    pub fn render(&mut self, out: &mut [[f32; 2]]) {
        let n = out.len();
        self.update(n as f64 / self.sr);

        let Scene { sr, ducks, voices, mix, send, headphones, .. } = self;
        mix.clear();
        mix.resize(n, [0.0; 2]);
        send.clear();
        send.resize(n, 0.0);
        // the head lets through what is below about 1.2 kHz (2c / a rad/s) unchanged
        let shade = 1.0 - (-2.0 * SPEED_OF_SOUND / HEAD_RADIUS / *sr).exp();
        for v in voices.iter_mut() {
            let pos = match v.source {
                Source::Duck(i) => ducks[i].pos,
                Source::Fixed(p) => p,
            };
            // glide from last block's placement to this one's, so moving ducks don't zipper
            let to = spatialize(pos, v.level, *sr, *headphones);
            let from = v.gains.unwrap_or(to);
            v.gains = Some(to);
            for k in 0..n {
                if v.delay > 0 {
                    v.delay -= 1;
                    continue;
                }
                let t = k as f64 / n as f64;
                let lerp = |a: f64, b: f64| a + (b - a) * t;
                let lowpass = lerp(from.lowpass, to.lowpass);
                // the reverb takes the sound as it reaches the middle of the head
                let x = v.samples.get(v.at).copied().unwrap_or(0.0);
                v.lowpassed += lowpass * (x - v.lowpassed);
                send[k] += v.lowpassed * lerp(from.send, to.send);
                for (c, ear) in v.ears.iter_mut().enumerate() {
                    let (a, b) = (from.ears[c], to.ears[c]);
                    let heard = if *headphones {
                        // each ear hears it a moment later, past the head
                        let x = sample_at(&v.samples, v.at as f64 - lerp(a.delay, b.delay));
                        ear.lowpassed += lowpass * (x - ear.lowpassed);
                        ear.shaded += shade * (ear.lowpassed - ear.shaded);
                        let bright = lerp(a.bright, b.bright);
                        bright * ear.lowpassed + (1.0 - bright) * ear.shaded
                    } else {
                        v.lowpassed
                    };
                    mix[k][c] += heard * lerp(a.gain, b.gain);
                }
                v.at += 1;
            }
        }
        // done once the far ear has heard the end
        voices.retain(|v| {
            let lag = v.gains.map_or(0.0, |g| g.ears[0].delay.max(g.ears[1].delay));
            (v.at as f64) < v.samples.len() as f64 + lag + 1.0
        });

        for k in 0..n {
            let wet = self.reverb.tick(self.send[k]);
            let water = self.water.tick(&mut self.rng);
            let fade_in = smoothstep((self.frame + k as u64) as f64 / (0.5 * self.sr));
            for c in 0..2 {
                let x = (self.mix[k][c] + wet[c] + water[c]) * MASTER * fade_in;
                out[k][c] = soft_limit(x) as f32;
            }
        }
        self.frame += n as u64;
    }

    fn update(&mut self, dt: f64) {
        for i in 0..self.ducks.len() {
            self.think(i, dt);
        }
        for i in 0..self.ducks.len() {
            self.steer(i, dt);
        }
        for i in 0..self.ducks.len() {
            let d = &mut self.ducks[i];
            d.call_timer -= dt;
            if let Some((_, left)) = &mut d.call {
                *left -= dt;
                if *left <= 0.0 {
                    d.call = None;
                }
            }
            if d.call_timer <= 0.0 && d.call.is_none() {
                self.call(i);
            }
        }
        // drips and bubbles, mostly along the near shore
        self.bubble_timer -= dt;
        if self.bubble_timer <= 0.0 {
            self.bubble_timer = self.rng.random_range(0.15..0.7);
            let band = if self.rng.random_bool(0.7) { (3.0, 8.0) } else { (3.0, 46.0) };
            let pos = pond_point(&mut self.rng, band);
            let level = self.rng.random_range(0.04..0.1);
            let samples = self.synth.bubble();
            self.emit(Source::Fixed(pos), samples, level);
        }
        // peas land with a plip on the water or a tap on the bank, and sink if left too long
        let mut landed = Vec::new();
        for pea in &mut self.peas {
            pea.age += dt;
            if pea.airborne > 0.0 {
                pea.airborne -= dt;
                if pea.airborne <= 0.0 {
                    landed.push(pea.pos);
                }
            }
        }
        self.peas.retain(|p| p.age < PEA_LIFETIME);
        for pos in landed {
            let (samples, level) = if in_pond(pos) {
                (self.synth.bubble(), PEA_PLOP)
            } else {
                (self.synth.tap(), PEA_TAP)
            };
            let level = level * self.rng.random_range(0.6..1.0);
            self.emit(Source::Fixed(pos), samples, level);
        }
    }

    fn emit(&mut self, source: Source, samples: Vec<f64>, level: f64) {
        let pos = match source {
            Source::Duck(i) => self.ducks[i].pos,
            Source::Fixed(p) => p,
        };
        let delay = (pos.len() / SPEED_OF_SOUND * self.sr) as usize;
        self.voices.push(Voice { samples, at: 0, delay, source, level, lowpassed: 0.0, ears: Default::default(), gains: None });
    }

    /// Decide what duck `i` is doing and where it is heading.
    fn think(&mut self, i: usize, dt: f64) {
        let d = &mut self.ducks[i];
        d.activity_left -= dt;
        d.offset_left -= dt;
        if let Some(t) = &mut d.notice_peas {
            *t -= dt;
            if *t <= 0.0 {
                d.notice_peas = None;
                if !self.peas.is_empty() {
                    // food! hurry over, and say so
                    d.activity = Activity::Feed;
                    d.eating = 0.0;
                    d.call_timer = d.call_timer.min(self.rng.random_range(0.2..1.5));
                }
            }
        }
        if let (Kind::Duckling, Some(m)) = (d.kind, d.leader) {
            // a duckling calls out as soon as it finds itself away from its
            // mother (though she is easy to keep in sight over the peas), and
            // keeps on until it is back at her side
            let far = if d.activity == Activity::Feed { 2.5 * LOST } else { LOST };
            let dist = (self.ducks[m].pos - self.ducks[i].pos).len();
            let d = &mut self.ducks[i];
            let lost = dist > far || d.lost && dist > 1.5;
            if lost && !d.lost {
                d.call_timer = d.call_timer.min(self.rng.random_range(0.1..0.5));
            }
            d.lost = lost;
            if lost && d.activity == Activity::Feed {
                // the peas can wait
                self.cue(i);
            }
        }
        let d = &self.ducks[i];
        if d.kind != Kind::Duckling && d.activity == Activity::Dabble && in_pond(d.pos) {
            self.dabble(i, dt);
        }
        let d = &mut self.ducks[i];
        match d.activity {
            Activity::Feed => {
                if d.eating > 0.0 {
                    d.eating -= dt;
                    d.target = d.pos;
                    return;
                }
                let pos = d.pos;
                match self.nearest_pea(pos) {
                    Some(k) if (self.peas[k].pos - pos).len() < 0.35 => {
                        self.peas.swap_remove(k);
                        let d = &mut self.ducks[i];
                        d.eating = self.rng.random_range(0.5..1.2);
                        let level = if d.kind == Kind::Duckling { DUCKLING_PECK } else { PECK };
                        let samples = self.synth.peck(in_pond(pos));
                        self.emit(Source::Duck(i), samples, level);
                    }
                    Some(k) => self.ducks[i].target = self.peas[k].pos,
                    // still in the air: head for where they are coming down
                    None if !self.peas.is_empty() => {
                        let sum = self.peas.iter().fold(Vec2::default(), |s, p| s + p.pos);
                        self.ducks[i].target = sum * (1.0 / self.peas.len() as f64);
                    }
                    None => self.stop_feeding(i),
                }
            }
            Activity::Stray => {
                if d.lost {
                    // stops and calls for her
                    d.target = d.pos;
                }
                if d.activity_left <= 0.0 {
                    self.cue(i);
                }
            }
            _ if d.leader.is_some() => self.keep_up(i, dt),
            Activity::Swim => {
                if (d.target - d.pos).len() < 0.7 || d.activity_left <= 0.0 {
                    self.next_activity(i);
                }
            }
            Activity::Dabble => {
                // potter about the feeding spot
                if d.offset_left <= 0.0 {
                    d.offset_left = self.rng.random_range(4.0..12.0);
                    let step = Vec2::new(self.rng.random_range(-1.0..1.0), self.rng.random_range(-1.0..1.0)).unit();
                    d.target = clamp_towards(d.pos, d.pos + step * self.rng.random_range(0.5..2.5));
                }
                if d.activity_left <= 0.0 {
                    self.next_activity(i);
                }
            }
            Activity::Rest | Activity::Preen => {
                if d.activity_left <= 0.0 {
                    self.next_activity(i);
                }
            }
        }
    }

    /// A dabbling duck sieves the surface in spells, upending or looking
    /// about in between.
    fn dabble(&mut self, i: usize, dt: f64) {
        let d = &mut self.ducks[i];
        d.splash_timer -= dt;
        if d.splash_timer > 0.0 {
            return;
        }
        let seconds = self.rng.random_range(2.0..7.0);
        d.splash_timer = seconds + self.rng.random_range(1.0..8.0);
        let samples = self.synth.dabbling(seconds);
        self.emit(Source::Duck(i), samples, DABBLING);
    }

    /// Keep station on the leader: ducklings in a line behind a mother on the
    /// move, darting about her while they feed and huddled up when they rest;
    /// a drake a little behind his mate.
    fn keep_up(&mut self, i: usize, dt: f64) {
        let leader = &self.ducks[self.ducks[i].leader.expect("followers have a leader")];
        let (lpos, lfacing) = (leader.pos, leader.facing);
        let rng = &mut self.rng;
        let d = &mut self.ducks[i];
        if d.offset_left <= 0.0 {
            let (spread, ahead, hold) = match (d.kind, d.activity) {
                (Kind::Drake, Activity::Swim) => (1.5, (-2.0, -0.5), (3.0, 8.0)),
                (Kind::Drake, _) => (2.0, (-2.0, 1.0), (10.0, 30.0)),
                (_, Activity::Rest) => (0.4, (-0.4, 0.3), (30.0, 90.0)),
                (_, Activity::Preen) => (1.0, (-1.0, 0.5), (5.0, 15.0)),
                _ => (1.5, (-1.5, 1.5), (2.0, 6.0)),
            };
            let offset = Vec2::new(rng.random_range(-spread..spread), rng.random_range(ahead.0..ahead.1));
            // a drake keeps beside his mate rather than on top of her
            d.offset = if d.kind == Kind::Drake { offset + offset.unit() * 0.6 } else { offset };
            d.offset_left = rng.random_range(hold.0..hold.1);
        }
        let offset = if d.lost {
            // straight back to her side
            Vec2::new(0.0, -0.5)
        } else if d.kind == Kind::Duckling && d.activity == Activity::Swim {
            Vec2::new(0.25 * d.offset.x, -0.8 - 0.55 * d.rank as f64)
        } else {
            d.offset
        };
        let right = Vec2::new(lfacing.y, -lfacing.x);
        d.target = clamp_towards(lpos, lpos + right * offset.x + lfacing * offset.y);
        // a feeding duckling now and then wanders off after insects
        if d.kind == Kind::Duckling && d.activity == Activity::Dabble && rng.random_bool((dt / STRAY_EVERY).min(1.0)) {
            let away = Vec2::new(rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0)).unit();
            d.activity = Activity::Stray;
            d.activity_left = rng.random_range(15.0..40.0);
            d.target = clamp_towards(d.pos, lpos + away * rng.random_range(5.0..9.0));
        }
    }

    /// Duck `i` takes its cue from what its leader is doing. Pairs and
    /// broods do the same thing about two thirds of the time (Pietz & Buhl,
    /// 1999): ducklings feed while their mother rests on the water, and a
    /// drake feeds less than his mate and keeps watch instead.
    fn cue(&mut self, i: usize) {
        let leader = &self.ducks[self.ducks[i].leader.expect("only followers take cues")];
        let (lead, ashore) = (leader.activity, !in_pond(leader.pos));
        let r: f64 = self.rng.random();
        let d = &mut self.ducks[i];
        d.activity = match (d.kind, lead) {
            (Kind::Duckling, Activity::Rest) if !ashore && r < 0.7 => Activity::Dabble,
            (Kind::Duckling, Activity::Preen) if r < 0.5 => Activity::Dabble,
            (Kind::Drake, Activity::Dabble) if r < 0.3 => Activity::Rest,
            // until they notice the peas themselves
            (_, Activity::Feed | Activity::Stray) => Activity::Swim,
            _ => lead,
        };
        d.offset_left = 0.0;
        d.splash_timer = self.rng.random_range(0.5..3.0);
    }

    /// The ducklings of hen `i`.
    fn young(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.ducks.len()).filter(move |&c| self.ducks[c].leader == Some(i) && self.ducks[c].kind == Kind::Duckling)
    }

    /// Duck `i` has started something new, and its followers go along.
    fn follow_lead(&mut self, i: usize) {
        for j in 0..self.ducks.len() {
            let d = &self.ducks[j];
            if d.leader == Some(i) && !matches!(d.activity, Activity::Feed | Activity::Stray) {
                self.cue(j);
            }
        }
    }

    /// What a duck that leads its own life (hens, the lone drake) does next.
    /// Bouts last minutes, and add up to about what mallards spend their day
    /// on: around 40 % feeding, 30 % resting, 15 % preening and the rest
    /// getting about.
    fn next_activity(&mut self, i: usize) {
        let has_young = self.young(i).next().is_some();
        let others: Vec<Activity> = (0..self.ducks.len())
            .filter(|&j| j != i && self.ducks[j].leader.is_none())
            .map(|j| self.ducks[j].activity)
            .collect();
        let rng = &mut self.rng;
        let d = &mut self.ducks[i];
        let ashore = !in_pond(d.pos);
        let r: f64 = rng.random();
        let pick = |a: f64, b: f64, x, y, z| {
            if r < a {
                x
            } else if r < b {
                y
            } else {
                z
            }
        };
        // what the others are doing is catching, so the pond has its lively
        // spells and its drowsy ones
        let others_doing = others
            .get(rng.random_range(0..others.len().max(1)))
            .copied()
            .filter(|a| matches!(a, Activity::Dabble | Activity::Rest | Activity::Preen) && rng.random_bool(0.35));
        d.activity = match (d.activity, others_doing) {
            // ducks come ashore to loaf
            (Activity::Swim, _) if ashore && r < 0.6 => Activity::Rest,
            (Activity::Swim, _) if ashore => Activity::Preen,
            (_, Some(theirs)) => theirs,
            (Activity::Swim, _) => pick(0.65, 0.85, Activity::Dabble, Activity::Rest, Activity::Preen),
            (Activity::Dabble, _) => pick(0.45, 0.75, Activity::Swim, Activity::Rest, Activity::Preen),
            // and preen once they wake
            (Activity::Rest, _) => pick(0.4, 0.75, Activity::Preen, Activity::Dabble, Activity::Swim),
            _ => pick(0.5, 0.7, Activity::Dabble, Activity::Rest, Activity::Swim),
        };
        d.target = d.pos;
        match d.activity {
            Activity::Swim => {
                if rng.random_bool(0.3) {
                    d.band = [NEAR, MID, FAR][rng.random_range(0..3)];
                }
                // ducks nearby may come ashore, a hen to brood her ducklings well away from people
                let shore_near = (d.pos - Vec2::new(0.0, 1.0)).len() < 20.0;
                d.target = if shore_near && rng.random_bool(if has_young { 0.4 } else { 0.15 }) {
                    bank_point(rng, if has_young { 5.0 } else { 2.0 })
                } else {
                    pond_point(rng, d.band)
                };
                d.activity_left = 180.0;
                if has_young {
                    // calling them together before setting off
                    d.gathering = true;
                    d.call_timer = d.call_timer.min(rng.random_range(0.3..1.2));
                }
            }
            Activity::Dabble => {
                d.activity_left = rng.random_range(60.0..240.0);
                d.offset_left = 0.0;
                d.splash_timer = rng.random_range(0.5..3.0);
            }
            Activity::Rest => d.activity_left = rng.random_range(60.0..240.0),
            _ => d.activity_left = rng.random_range(30.0..120.0),
        }
        self.follow_lead(i);
    }

    fn steer(&mut self, i: usize, dt: f64) {
        let d = &self.ducks[i];
        let pos = d.pos;
        let lead = d.leader.filter(|_| !matches!(d.activity, Activity::Feed | Activity::Stray));
        let leader = lead.map(|l| (self.ducks[l].pos, self.ducks[l].vel));
        // a hen waits for stragglers
        let waiting = self
            .young(i)
            .map(|c| &self.ducks[c])
            .any(|c| c.lost || c.activity != Activity::Feed && (c.pos - pos).len() > 3.5);
        let has_young = self.young(i).next().is_some();
        // grown ducks keep a little apart, mates included
        let crowd = if d.kind == Kind::Duckling {
            Vec2::default()
        } else {
            self.ducks.iter().filter(|o| o.kind != Kind::Duckling).fold(Vec2::default(), |push, o| {
                let away = pos - o.pos;
                push + away.unit() * (0.15 * (1.0 - away.len() / 0.6)).max(0.0)
            })
        };
        let rng = &mut self.rng;
        let d = &mut self.ducks[i];
        let to = d.target - d.pos;
        let dist = to.len();
        let desired = match leader {
            // match the leader's pace while closing in; a duckling left behind
            // bursts across the surface to catch up
            Some((lpos, lvel)) => {
                let (paddle, burst) =
                    if d.kind == Kind::Duckling { (DUCKLING_HULL_SPEED, DUCKLING_BURST) } else { (0.6, HULL_SPEED) };
                let cap = if d.lost || (lpos - d.pos).len() > 3.0 {
                    burst
                } else if matches!(d.activity, Activity::Rest | Activity::Preen) {
                    0.15
                } else {
                    paddle
                };
                let v = lvel + to * 0.8;
                if v.len() > cap { v.unit() * cap } else { v }
            }
            None => {
                let speed = match d.activity {
                    Activity::Swim if waiting => 0.0,
                    Activity::Swim if has_young => d.cruise.min(BROOD_PACE) * (dist / 1.5).min(1.0),
                    Activity::Swim => d.cruise * (dist / 1.5).min(1.0),
                    Activity::Stray => 0.2 * (dist / 0.5).min(1.0),
                    Activity::Dabble => 0.12 * (dist / 0.5).min(1.0),
                    Activity::Rest | Activity::Preen => 0.05 * dist.min(1.0),
                    Activity::Feed if d.eating > 0.0 => 0.05 * dist.min(1.0),
                    Activity::Feed => (1.8 * d.cruise).min(1.0) * (dist / 0.8).min(1.0),
                };
                to.unit() * speed
            }
        };
        let wander = Vec2::new(rng.sample(StandardNormal), rng.sample(StandardNormal)) * 0.04;
        d.vel = d.vel + (desired + crowd + wander - d.vel) * (2.5 * dt).min(1.0);
        // slide along the shore, or round the listener, rather than bouncing off
        let step = d.vel * dt;
        let out = d.pos.unit();
        let round = step - out * step.dot(out);
        match [step, Vec2::new(step.x, 0.0), Vec2::new(0.0, step.y), round].into_iter().find(|&s| allowed(d.pos + s)) {
            Some(s) => {
                d.pos = d.pos + s;
                d.vel = s * (1.0 / dt);
            }
            None => {
                d.vel = Vec2::default();
                if d.activity == Activity::Swim && lead.is_none() {
                    d.activity_left = 0.0; // pick somewhere else
                }
            }
        }
        if d.vel.len() > 0.05 {
            d.facing = d.vel.unit();
        }
    }

    /// Duck `i` calls, and may set others off answering. Ducks are quiet
    /// for minutes at a time unless something is going on (food, a lost
    /// duckling, a mate out of sight), and next to silent asleep.
    fn call(&mut self, i: usize) {
        let me = &self.ducks[i];
        let (kind, activity, pos, voice, answering, gathering, lost) =
            (me.kind, me.activity, me.pos, me.voice, me.answering, me.gathering, me.lost);
        let young: Vec<usize> = self.young(i).collect();
        let lost_young = young.iter().any(|&c| self.ducks[c].lost);
        // a drake calls after a mate gone out of reach
        let mate_away = kind == Kind::Drake && me.leader.is_some_and(|m| (self.ducks[m].pos - pos).len() > 8.0);
        let mother = (kind == Kind::Duckling).then(|| me.leader.expect("ducklings have a mother"));
        let asleep = activity == Activity::Rest && !answering && !lost && !lost_young;
        let rng = &mut self.rng;
        let synth = &mut self.synth;
        let next = match (kind, activity) {
            (Kind::Hen, _) if lost_young => rng.random_range(1.5..3.0),
            (Kind::Duckling, _) if lost => rng.random_range(0.2..1.0),
            (Kind::Duckling, Activity::Feed) => rng.random_range(1.5..5.0),
            (Kind::Duckling, _) => rng.random_range(4.0..15.0),
            (_, Activity::Feed) => rng.random_range(2.5..6.0),
            (Kind::Drake, _) if mate_away => rng.random_range(8.0..20.0),
            (Kind::Drake, _) => rng.random_range(45.0..180.0),
            (_, Activity::Dabble) => rng.random_range(15.0..60.0),
            _ => rng.random_range(20.0..90.0),
        };
        // asleep, a duck lets nearly all of them go by
        if asleep && rng.random_bool(if kind == Kind::Duckling { 0.8 } else { 0.9 }) {
            self.ducks[i].call_timer = next;
            return;
        }
        let mut social = false;
        let mut brood = false;
        let (label, samples, level) = match kind {
            Kind::Hen => {
                let (f0, f2) = voice;
                let r: f64 = rng.random();
                let decrescendo = |rng: &mut StdRng, synth: &mut Synth| {
                    ("QUACK quack quack", synth.decrescendo(rng.random_range(5..=7), f0, f2), DECRESCENDO)
                };
                let chatter = |rng: &mut StdRng, synth: &mut Synth| {
                    ("wak-wak-wak", synth.chatter(rng.random_range(1.5..3.5), f0, f2), CHATTER)
                };
                if lost_young || gathering {
                    // calling her brood together, or back to her
                    brood = true;
                    let notes = if lost_young { rng.random_range(6..=10) } else { rng.random_range(3..=6) };
                    ("kwek-kwek", synth.brood_call(notes, f0, f2), BROOD_CALL)
                } else if matches!(activity, Activity::Dabble | Activity::Feed) && rng.random_bool(0.6) {
                    // the quiet "wak-wak" of a hen with her bill in the food
                    chatter(rng, synth)
                } else if r < 0.45 {
                    let (dur, rasp) = (rng.random_range(0.2..0.24), rng.random_range(0.4..0.6));
                    ("quack", synth.quack(dur, f0, f2, rasp), QUACK)
                } else if r < 0.7 {
                    social = true;
                    ("quack quack", synth.series(rng.random_range(2..=3), f0, f2), SERIES)
                } else if r < 0.9 {
                    social = true;
                    decrescendo(rng, synth)
                } else {
                    chatter(rng, synth)
                }
            }
            Kind::Drake => {
                let samples = synth.drake(rng.random_range(1..=3), rng.random_range(0.9..1.4), voice.0);
                ("rhaeb", samples, RHAEB)
            }
            // distress peeps when lost; otherwise soft contact notes
            Kind::Duckling if lost => ("peep peep", synth.peeping(rng.random_range(1.0..2.5), voice.0), PEEPS),
            Kind::Duckling => ("pip-pip", synth.contact(rng.random_range(2..=5), voice.0), CONTACT),
        };
        let d = &mut self.ducks[i];
        d.call = Some((label, samples.len() as f64 / self.sr));
        d.call_timer = next;
        d.answering = false;
        d.gathering = false;
        self.emit(Source::Duck(i), samples, level);

        if let Some(m) = mother.filter(|_| lost) {
            // she answers a lost duckling, as soon as she has her head up
            let m = &mut self.ducks[m];
            m.call_timer = m.call_timer.min(self.rng.random_range(0.5..2.5));
        }
        if brood {
            // a duckling that has wandered off heads back when it hears her
            for c in young {
                if self.ducks[c].activity == Activity::Stray {
                    self.cue(c);
                }
            }
        }
        if social && !answering {
            for (j, other) in self.ducks.iter_mut().enumerate() {
                let dist = (other.pos - pos).len();
                if j == i || other.kind == Kind::Duckling || other.activity == Activity::Rest || dist > 40.0 {
                    continue;
                }
                // her mate usually answers, others now and then, more often when close
                let p = if other.leader == Some(i) { 0.6 } else { 0.3 * (1.0 - dist / 40.0) };
                if self.rng.random_bool(p) {
                    other.answering = true;
                    other.call_timer = other.call_timer.min(self.rng.random_range(0.8..2.5));
                }
            }
        }
    }
}

/// Three hens (two with ducklings), a drake escorting his mate, and a lone drake.
fn flock(rng: &mut StdRng) -> Vec<Duck> {
    let mut ducks = Vec::new();
    let hen = |ducks: &mut Vec<Duck>, rng: &mut StdRng, name, voice, band| {
        let mut d = Duck::new(name, Kind::Hen, voice, pond_point(rng, band), rng);
        d.band = band;
        ducks.push(d);
        ducks.len() - 1
    };
    let mabel = hen(&mut ducks, rng, "Mabel", (430.0, 2300.0), NEAR);
    let clover = hen(&mut ducks, rng, "Clover", (465.0, 2550.0), MID);
    let hazel = hen(&mut ducks, rng, "Hazel", (405.0, 2200.0), FAR);

    let beside = |ducks: &Vec<Duck>, i: usize, offset| clamp_towards(ducks[i].pos, ducks[i].pos + offset);
    let mut rufus = Duck::new("Rufus", Kind::Drake, (1850.0, 0.0), beside(&ducks, clover, Vec2::new(1.5, -1.0)), rng);
    rufus.leader = Some(clover);
    ducks.push(rufus);
    let mut bramble = Duck::new("Bramble", Kind::Drake, (1750.0, 0.0), pond_point(rng, FAR), rng);
    bramble.band = FAR;
    ducks.push(bramble);

    for (mother, names) in [(mabel, &["Pip", "Dot", "Bean", "Fig"][..]), (hazel, &["Nib", "Tuft", "Sprout"][..])] {
        for (rank, &name) in names.iter().enumerate() {
            let top = rng.random_range(4200.0..4700.0);
            let offset = Vec2::new(rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0));
            let mut d = Duck::new(name, Kind::Duckling, (top, 0.0), beside(&ducks, mother, offset), rng);
            d.leader = Some(mother);
            d.rank = rank;
            ducks.push(d);
        }
    }
    ducks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dB taken off at `f` by the one-pole lowpass with coefficient `a`.
    fn lowpass_loss(a: f64, f: f64, sr: f64) -> f64 {
        let p = 1.0 - a;
        -10.0 * (a * a / (1.0 - 2.0 * p * (TAU * f / sr).cos() + p * p)).log10()
    }

    #[test]
    fn distant_ducks_lose_their_highs_like_the_air_does() {
        // ISO 9613-1 at 20 °C and 70 % humidity, dB per metre
        let air = [(2000.0, 0.009), (4000.0, 0.0231), (8000.0, 0.0776)];
        let sr = 48000.0;
        for r in [10.0, 25.0, 45.0] {
            let a = spatialize(Vec2::new(0.0, r), 1.0, sr, false).lowpass;
            for (f, db_per_m) in air {
                let (loss, iso) = (lowpass_loss(a, f, sr), db_per_m * r);
                assert!((loss - iso).abs() < 0.1 + 0.25 * iso, "{r} m, {f} Hz: {loss:.2} dB, ISO {iso:.2} dB");
            }
        }
    }

    #[test]
    fn on_headphones_the_near_ear_hears_first_and_brighter() {
        let sr = 48000.0;
        for degrees in [0.0, 30.0, 60.0, 90.0, 120.0, 180.0] {
            let bearing = f64::to_radians(degrees);
            let [left, right] = spatialize(Vec2::new(bearing.sin(), bearing.cos()) * 10.0, 1.0, sr, true).ears;
            // Woodworth's time difference for a sphere, from the angle off the front-back axis
            let lateral = bearing.sin().asin();
            let itd = HEAD_RADIUS / SPEED_OF_SOUND * (lateral + lateral.sin());
            assert!(((left.delay - right.delay) / sr - itd).abs() < 1e-9, "{degrees}°");
            assert_eq!(left.gain, right.gain);
            assert!(right.bright > left.bright || lateral.abs() < 1e-9, "{degrees}°");
        }
    }

    /// Seconds the pond moves on per block of audio at 48 kHz.
    const STEP: f64 = 512.0 / 48000.0;

    /// Let the ducks get on with it for `seconds`, without rendering what
    /// they sound like, while `watch` looks on.
    fn simulate(scene: &mut Scene, seconds: f64, mut watch: impl FnMut(&Scene)) {
        for _ in 0..(seconds / STEP) as usize {
            scene.update(STEP);
            scene.voices.clear();
            watch(scene);
        }
    }

    fn pond(seed: u64) -> Scene {
        Scene::new(16000.0, StdRng::seed_from_u64(seed), false)
    }

    fn calling(duck: &Duck, label: &str) -> bool {
        duck.call.is_some_and(|(l, _)| l == label)
    }

    #[test]
    fn a_lost_duckling_peeps_until_its_mother_calls_it_back() {
        let mut scene = pond(7);
        simulate(&mut scene, 10.0, |_| {});
        // the last in line behind a mother setting off across the pond...
        let young = (0..scene.ducks.len()).rfind(|&j| scene.ducks[j].kind == Kind::Duckling).unwrap();
        let mother = scene.ducks[young].leader.unwrap();
        let m = &mut scene.ducks[mother];
        (m.activity, m.activity_left, m.target) = (Activity::Swim, 180.0, POND_CENTER + Vec2::new(0.0, 15.0));
        scene.follow_lead(mother);
        // ...has dropped 6 m behind her, where it waits to hear her
        let behind = scene.ducks[mother].pos - scene.ducks[mother].facing * 6.0;
        assert!(allowed(behind));
        let d = &mut scene.ducks[young];
        d.pos = behind;
        d.activity = Activity::Stray;
        d.activity_left = 60.0;

        let (mut peeped, mut called, mut back, mut t) = (false, false, None, 0.0);
        simulate(&mut scene, 30.0, |s| {
            t += STEP;
            peeped |= calling(&s.ducks[young], "peep peep");
            called |= calling(&s.ducks[mother], "kwek-kwek");
            if peeped && !s.ducks[young].lost && back.is_none() {
                back = Some(t);
            }
        });
        assert!(peeped, "the duckling never called for its mother");
        assert!(called, "its mother never called it back");
        assert!(back.is_some_and(|t| t < 15.0), "back with her after {back:?} s");
    }

    #[test]
    fn ducks_give_up_on_the_peas_once_they_are_gone() {
        for seed in [2, 8] {
            let mut scene = pond(seed);
            simulate(&mut scene, 30.0, |_| {});
            scene.handle(Command::ScatterPeas);
            let mut gone = false;
            let mut after = vec![0.0; scene.ducks.len()];
            simulate(&mut scene, 120.0, |s| {
                gone |= s.peas.is_empty();
                for (i, d) in s.ducks.iter().enumerate() {
                    // only whoever got the last one may still be chewing it
                    after[i] = if s.peas.is_empty() && d.activity == Activity::Feed { after[i] + STEP } else { 0.0 };
                    assert!(after[i] < 1.3, "seed {seed}: {} still after the peas {:.1} s on", d.name, after[i]);
                }
            });
            assert!(gone, "seed {seed}: the peas were never all eaten");
        }
    }

    #[test]
    fn ducks_keep_mallard_hours_and_ducklings_keep_close() {
        let mut scene = pond(11);
        let n = scene.ducks.len();
        let minutes = 20.0;
        let mut was = scene.ducks.iter().map(|d| (d.activity, d.call.is_some())).collect::<Vec<_>>();
        let (mut hens, mut swimming, mut feeding, mut bouts, mut adult_calls) = (0.0, 0.0, 0.0, 0, 0);
        let (mut young, mut racing, mut astray, mut longest_lost) = (0.0, 0.0, 0.0, 0.0f64);
        let mut lost_for = vec![0.0; n];
        simulate(&mut scene, minutes * 60.0, |s| {
            for (i, d) in s.ducks.iter().enumerate() {
                let starts_calling = d.call.is_some() && !was[i].1;
                match d.kind {
                    Kind::Duckling => {
                        young += STEP;
                        // paddling stays under hull speed; bursts are brief
                        if d.vel.len() > DUCKLING_HULL_SPEED + 0.07 {
                            racing += STEP;
                        }
                        if (s.ducks[d.leader.unwrap()].pos - d.pos).len() > LOST {
                            astray += STEP;
                        }
                        lost_for[i] = if d.lost { lost_for[i] + STEP } else { 0.0 };
                        longest_lost = longest_lost.max(lost_for[i]);
                        if starts_calling {
                            assert_eq!(calling(d, "peep peep"), d.lost, "{} peeps only when lost", d.name);
                        }
                    }
                    kind => {
                        adult_calls += starts_calling as usize;
                        if kind == Kind::Hen {
                            hens += STEP;
                            swimming += if d.activity == Activity::Swim { STEP } else { 0.0 };
                            feeding += if d.activity == Activity::Dabble { STEP } else { 0.0 };
                            bouts += (d.activity != was[i].0) as usize;
                        }
                    }
                }
                was[i] = (d.activity, d.call.is_some());
            }
        });
        // mallards spend some 40 % of the day feeding and only a tenth or so
        // getting about, in bouts of minutes
        let (swimming, feeding) = (swimming / hens, feeding / hens);
        assert!(swimming < 0.25, "hens swam {:.0} % of the time", 100.0 * swimming);
        assert!(feeding > 0.2, "hens fed {:.0} % of the time", 100.0 * feeding);
        assert!(hens / bouts as f64 > 60.0, "hens' bouts lasted {:.0} s", hens / bouts as f64);
        let per_minute = adult_calls as f64 / minutes;
        assert!((1.0..10.0).contains(&per_minute), "{per_minute:.1} calls a minute");
        assert!(racing / young < 0.02, "ducklings raced {:.1} % of the time", 100.0 * racing / young);
        assert!(astray / young < 0.03, "ducklings strayed {:.1} % of the time", 100.0 * astray / young);
        assert!(longest_lost < 20.0, "a duckling was lost for {longest_lost:.0} s");
    }
}
