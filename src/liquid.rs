//! The liquid of the sidebar effect (V): small drops of liquid floating weightless all over the
//! sidebar, one for each band of the sound visualizer, each moving to its own part of the music.
//!
//! There is no gravity and no one body. The 48 drops sit spread over the panel, like the bars of
//! the visualizer laid out in rows: bass at the bottom, treble at the top. Each drop swells with
//! its band and trembles with it: the louder the band, the larger the drop and the harder it
//! shakes, and higher bands shake faster. Drops near each other flow together when they swell
//! (metaballs), so loud passages join them into streams and quiet ones leave them apart. A bass
//! beat sends a ripple out through the drops; their places drift slowly, so the pattern is never
//! fixed. Each drop is held to its place by a soft spring.
//!
//! Space is in cell widths; a cell is about twice as tall as wide, so y is in half rows and the
//! drops look round on screen. It is cheap: 48 drops and at most 30 particles, 60 steps a second,
//! only while the music plays; drawing adds each drop only near itself (`grid`).

use crate::theme::Liquid as Kind;
use crate::viz::{Frame, BANDS};
use std::f32::consts::TAU;
use std::time::Instant;

/// Steps of the simulation per second (fixed, so it is stable at any frame rate).
const RATE: f32 = 60.0;
/// Drops: one per band.
pub const DROPS: usize = BANDS;
/// Drops in a row of the layout (8 rows of 6).
const PER_ROW: usize = 6;
/// At most this many particles at a time.
const MAX_PARTICLES: usize = 30;
/// A drop adds to the field only this many of its radii around it (beyond, it adds under 1/16).
const REACH: f32 = 4.0;

/// How one liquid moves.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// A drop's radius when its band is silent and at full sound, as a share of the space
    /// between drops.
    pub small: f32,
    pub large: f32,
    /// The spring that holds a drop at its place, and how fast its shaking dies down.
    pub spring: f32,
    pub damping: f32,
    /// How hard a loud band shakes its drop, in cells per second.
    pub shake: f32,
    /// How hard a bass beat's ripple pushes, in cells per second.
    pub kick: f32,
    /// How far the drops' places drift, as a share of the space between drops.
    pub drift: f32,
    /// Particles born per second at full sound (some kinds also without sound).
    pub rate: f32,
}

pub fn params(kind: Kind) -> Params {
    let base = Params { small: 0.16, large: 0.4, spring: 30.0, damping: 4.0, shake: 9.0, kick: 9.0, drift: 0.35, rate: 1.0 };
    match kind {
        // Thick: big soft drops that sway slowly.
        Kind::Honey => Params { small: 0.2, large: 0.42, spring: 14.0, damping: 6.0, shake: 4.0, kick: 5.0, drift: 0.25, rate: 0.0 },
        // Fizzy: small, jumpy drops.
        Kind::Soda => Params { small: 0.12, large: 0.35, spring: 40.0, damping: 3.0, shake: 13.0, rate: 5.0, ..base },
        Kind::Lava => Params { small: 0.2, large: 0.45, spring: 16.0, damping: 5.0, shake: 5.0, kick: 6.0, drift: 0.45, rate: 0.6 },
        // Ink: tiny drops that spread wide on a beat.
        Kind::Ink => Params { small: 0.1, large: 0.37, kick: 13.0, rate: 0.0, ..base },
        // Ice: calm and crisp, little shaking.
        Kind::Ice => Params { small: 0.14, large: 0.34, spring: 45.0, damping: 5.0, shake: 6.0, kick: 7.0, drift: 0.2, rate: 0.4 },
        Kind::Dew => Params { small: 0.14, large: 0.38, shake: 8.0, rate: 2.5, ..base },
        Kind::Neon => Params { small: 0.12, large: 0.38, spring: 38.0, damping: 3.0, shake: 12.0, kick: 11.0, rate: 0.0, ..base },
    }
}

/// One drop of the liquid.
#[derive(Debug, Clone, Copy)]
pub struct Drop {
    /// Place and speed, in cells (x) and half rows (y), from the panel's top left.
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
    /// Its radius now.
    pub r: f32,
    /// Its place in the layout (it drifts around it), and a phase for the drift.
    home_x: f32,
    home_y: f32,
    phase: f32,
}

/// What a particle is; how it moves and looks depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bit {
    /// Wanders inside a drop and pops at its skin (soda fizz).
    Bubble,
    /// Wanders slowly inside a drop, bright (lava).
    Blob,
    /// A shard of ice floating free in the panel, bouncing off the walls (ice).
    Floe,
    /// Twinkles inside a drop a moment (dew).
    Twinkle,
    /// Flies off a drop on a beat, and fades (neon).
    Spark,
}

#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub bit: Bit,
    /// Place and speed, like a drop's.
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Seconds lived, and the most it may live.
    pub age: f32,
    pub life: f32,
}

#[derive(Debug, Clone)]
pub struct Liquid {
    /// The panel, in cells and half rows.
    w: f32,
    h: f32,
    pub drops: Vec<Drop>,
    /// The space between neighbouring drops' places.
    pub space: f32,
    /// Slow average of the bass, to find beats over it.
    bass_avg: f32,
    /// Seconds until the next beat may count.
    beat_wait: f32,
    pub particles: Vec<Particle>,
    /// Particles owed, a fraction carried between steps.
    owed: f32,
    seed: u32,
    /// Time not yet simulated.
    left: f32,
    /// Seconds simulated in all.
    pub time: f32,
    at: Option<Instant>,
}

impl Default for Liquid {
    fn default() -> Self {
        Liquid {
            w: 0.0,
            h: 0.0,
            drops: Vec::new(),
            space: 0.0,
            bass_avg: 0.0,
            beat_wait: 0.0,
            particles: Vec::new(),
            owed: 0.0,
            seed: 0x9e37_79b9,
            left: 0.0,
            time: 0.0,
            at: None,
        }
    }
}

impl Liquid {
    /// Moves the liquid on to now, in a panel of `cols` × `rows` cells, to the sound of `frame`.
    pub fn update(&mut self, frame: &Frame, cols: u16, rows: u16, kind: Kind) {
        let now = Instant::now();
        let dt = self.at.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.at = Some(now);
        self.advance(frame, cols, rows, kind, dt);
    }

    /// Moves the liquid on by `dt` seconds.
    pub fn advance(&mut self, frame: &Frame, cols: u16, rows: u16, kind: Kind, dt: f32) {
        self.fit(cols as f32, rows as f32 * 2.0, params(kind));
        self.left += dt;
        let step = 1.0 / RATE;
        while self.left >= step {
            self.left -= step;
            self.step(frame, kind, step);
        }
    }

    /// The liquid's field at (x, y): over 1 inside a drop, under 1 outside. `scale` (0..1)
    /// shrinks every drop (the effect growing in or going away).
    pub fn field(&self, x: f32, y: f32, scale: f32) -> f32 {
        self.drops
            .iter()
            .map(|d| {
                let r = d.r * scale;
                let d2 = (x - d.x).powi(2) + (y - d.y).powi(2);
                if d2 > (r * REACH).powi(2) { 0.0 } else { r * r / (d2 + 0.01) }
            })
            .sum()
    }

    /// The field at a grid of points: `per_x` × `per_y` points in each cell of a panel of
    /// `cols` × `rows` cells, row by row. Each drop adds only to the points near it, so this is
    /// much cheaper than asking `field` at every point.
    pub fn grid(&self, cols: u16, rows: u16, per_x: usize, per_y: usize, scale: f32) -> Vec<f32> {
        let (gw, gh) = (cols as usize * per_x, rows as usize * per_y);
        let mut g = vec![0.0f32; gw * gh];
        // Point (i, j) sits at x = (i + 0.5) / per_x cells, y = (j + 0.5) * 2 / per_y half rows.
        let (sx, sy) = (1.0 / per_x as f32, 2.0 / per_y as f32);
        for d in &self.drops {
            let r = d.r * scale;
            let reach = r * REACH;
            let i0 = ((d.x - reach) / sx).floor().max(0.0) as usize;
            let i1 = (((d.x + reach) / sx).ceil().max(0.0) as usize).min(gw);
            let j0 = ((d.y - reach) / sy).floor().max(0.0) as usize;
            let j1 = (((d.y + reach) / sy).ceil().max(0.0) as usize).min(gh);
            for j in j0..j1 {
                let dy = (j as f32 + 0.5) * sy - d.y;
                for i in i0..i1 {
                    let dx = (i as f32 + 0.5) * sx - d.x;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= reach * reach {
                        g[j * gw + i] += r * r / (d2 + 0.01);
                    }
                }
            }
        }
        g
    }

    /// True if (x, y) is inside a drop.
    pub fn inside(&self, x: f32, y: f32) -> bool {
        self.field(x, y, 1.0) > 1.0
    }

    /// A new panel size: the drops are laid out over it again (they keep their speeds).
    fn fit(&mut self, w: f32, h: f32, p: Params) {
        if self.w == w && self.h == h {
            return;
        }
        let rows = DROPS.div_ceil(PER_ROW);
        let (gx, gy) = (w / PER_ROW as f32, h / rows as f32);
        self.space = gx.min(gy);
        let fresh = self.drops.is_empty();
        if fresh {
            self.drops = vec![Drop { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, r: 0.0, home_x: 0.0, home_y: 0.0, phase: 0.0 }; DROPS];
        }
        for b in 0..DROPS {
            // Band b: bass in the bottom row, treble in the top one; every other row is set half
            // a place over, so the drops make a honeycomb, not a grid.
            let (row, col) = (b / PER_ROW, b % PER_ROW);
            let shift = if row % 2 == 1 { 0.5 } else { 0.0 };
            let jitter = ((b as f32 * 12.9898).sin() * 43_758.547).fract() - 0.5;
            let hx = ((col as f32 + 0.5 + shift * 0.5 + jitter * 0.3) * gx).min(w - 1.0);
            let hy = h - (row as f32 + 0.5 + jitter * 0.25) * gy;
            let d = &mut self.drops[b];
            if fresh {
                d.x = hx;
                d.y = hy;
                d.r = self.space * p.small;
                d.phase = (b as f32 * 2.399_963) % TAU;
            } else {
                d.x += hx - d.home_x;
                d.y += hy - d.home_y;
            }
            d.home_x = hx;
            d.home_y = hy;
        }
        self.w = w;
        self.h = h;
    }

    fn random(&mut self) -> f32 {
        // xorshift
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn step(&mut self, frame: &Frame, kind: Kind, dt: f32) {
        let p = params(kind);
        self.time += dt;
        let bass = frame.levels[..6].iter().sum::<f32>() / 6.0;

        // A bass beat: a ripple runs out from a random place, pushing the drops away from it.
        self.bass_avg += (bass - self.bass_avg) * (dt * 1.5).min(1.0);
        self.beat_wait -= dt;
        let jump = bass - self.bass_avg;
        let mut ripple = None;
        if jump > 0.07 && bass > 0.2 && self.beat_wait <= 0.0 {
            let strength = (jump * 4.0).min(1.0);
            ripple = Some((self.random() * self.w, self.h * (0.5 + self.random() * 0.5), strength));
            self.beat_wait = 0.28;
        }
        let space = self.space.max(0.5);
        let damp = (1.0 - p.damping * dt).max(0.0);
        let t = self.time;
        let mut sparks = Vec::new();
        for b in 0..self.drops.len() {
            let level = frame.levels[b.min(BANDS - 1)];
            let shake = self.random() - 0.5;
            let shake2 = self.random() - 0.5;
            let d = &mut self.drops[b];
            // It swells with its band at once and shrinks back a little slower.
            let target = space * (p.small + (p.large - p.small) * level);
            d.r += (target - d.r) * if target > d.r { (dt * 18.0).min(1.0) } else { (dt * 6.0).min(1.0) };
            // Its place drifts in a slow loop, so the pattern is never fixed.
            let (hx, hy) = (
                (d.home_x + (t * 0.21 + d.phase).sin() * space * p.drift).clamp(d.r, (self.w - d.r).max(d.r)),
                (d.home_y + (t * 0.17 + d.phase * 1.3).cos() * space * p.drift * 1.4).clamp(d.r, (self.h - d.r).max(d.r)),
            );
            // Trembling: a loud band shakes its drop, higher bands faster (a quick wobble on top
            // of random jolts).
            let freq = 6.0 + b as f32 * 0.5;
            let wobble = (t * freq + d.phase).sin() * level * p.shake;
            let mut ax = p.spring * (hx - d.x) + shake * level * p.shake * 6.0 + wobble;
            let mut ay = p.spring * (hy - d.y) + shake2 * level * p.shake * 6.0 + (t * freq * 1.3 + d.phase).cos() * level * p.shake;
            if let Some((rx, ry, strength)) = ripple {
                let (dx, dy) = (d.x - rx, d.y - ry);
                let dist = dx.hypot(dy).max(1.0);
                // Near drops get the most; the push falls off with distance.
                let push = p.kick * strength * 180.0 / (1.0 + dist / space);
                ax += dx / dist * push;
                ay += dy / dist * push;
                if kind == Kind::Neon && dist < space * 2.0 {
                    sparks.push((d.x, d.y, dx / dist, dy / dist));
                }
            }
            d.vx = (d.vx + ax * dt) * damp;
            d.vy = (d.vy + ay * dt) * damp;
            d.x += d.vx * dt;
            d.y += d.vy * dt;
            // The walls: a drop bounces off them, losing some speed.
            if d.x < 0.0 || d.x > self.w {
                d.x = d.x.clamp(0.0, self.w);
                d.vx = -d.vx * 0.5;
            }
            if d.y < 0.0 || d.y > self.h {
                d.y = d.y.clamp(0.0, self.h);
                d.vy = -d.vy * 0.5;
            }
        }
        for (x, y, ux, uy) in sparks {
            let s = 6.0 + self.random() * 6.0;
            self.spawn(Bit::Spark, x, y, ux * s, uy * s, 0.7);
        }
        self.particles_step(frame, kind, p, dt);
    }

    fn spawn(&mut self, bit: Bit, x: f32, y: f32, vx: f32, vy: f32, life: f32) {
        if self.particles.len() < MAX_PARTICLES {
            self.particles.push(Particle { bit, x, y, vx, vy, age: 0.0, life });
        }
    }

    /// A drop picked at random, more often a loud (large) one: its place.
    fn loud_drop(&mut self) -> (f32, f32) {
        let mut best = (0.0, (self.w / 2.0, self.h / 2.0));
        for _ in 0..3 {
            let i = (self.random() * self.drops.len() as f32) as usize % self.drops.len().max(1);
            if let Some(d) = self.drops.get(i) {
                if d.r > best.0 {
                    best = (d.r, (d.x, d.y));
                }
            }
        }
        best.1
    }

    fn particles_step(&mut self, frame: &Frame, kind: Kind, p: Params, dt: f32) {
        let energy = frame.levels.iter().sum::<f32>() / BANDS as f32;
        // Particles of the kind: some come also in silence, slower.
        let quiet = match kind {
            Kind::Lava | Kind::Ice | Kind::Dew => 0.4,
            _ => 0.0,
        };
        self.owed += p.rate * (energy + quiet) * dt;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            match kind {
                Kind::Soda => {
                    let (x, y) = self.loud_drop();
                    let a = self.random() * TAU;
                    self.spawn(Bit::Bubble, x, y, a.cos() * 1.5, a.sin() * 1.5, 3.0);
                }
                Kind::Lava => {
                    let (x, y) = self.loud_drop();
                    self.spawn(Bit::Blob, x, y, 0.0, 0.0, 2.0);
                }
                Kind::Dew => {
                    let (x, y) = self.loud_drop();
                    let life = 0.5 + self.random() * 0.6;
                    self.spawn(Bit::Twinkle, x, y, 0.0, 0.0, life);
                }
                Kind::Ice if self.particles.iter().filter(|q| q.bit == Bit::Floe).count() < 4 => {
                    let (x, y) = (self.random() * self.w, self.random() * self.h);
                    if !self.inside(x, y) {
                        let a = self.random() * TAU;
                        let life = 14.0 + self.random() * 8.0;
                        self.spawn(Bit::Floe, x, y, a.cos() * 1.2, a.sin() * 1.2, life);
                    }
                }
                // Honey, ink and neon have none (neon has its sparks on beats).
                _ => {}
            }
        }
        let (w, h) = (self.w, self.h);
        let mut out = std::mem::take(&mut self.particles);
        for q in &mut out {
            q.age += dt;
            match q.bit {
                Bit::Bubble => {
                    // Wanders, and pops when it leaves the liquid.
                    let turn = (q.age * 3.0 + q.x).sin() * 3.0;
                    q.vx += turn * dt;
                    q.vy -= turn * dt;
                    if q.age > 0.2 && !self.inside(q.x, q.y) {
                        q.age = q.life;
                    }
                }
                Bit::Spark => {
                    q.vx *= 1.0 - 1.5 * dt;
                    q.vy *= 1.0 - 1.5 * dt;
                }
                Bit::Blob | Bit::Twinkle | Bit::Floe => {}
            }
            q.x += q.vx * dt;
            q.y += q.vy * dt;
            // Everything free in the panel bounces off its walls.
            if q.x < 0.0 || q.x > w {
                q.vx = -q.vx * 0.8;
                q.x = q.x.clamp(0.0, w);
            }
            if q.y < 0.0 || q.y > h {
                q.vy = -q.vy * 0.8;
                q.y = q.y.clamp(0.0, h);
            }
        }
        out.retain(|q| q.age < q.life);
        self.particles = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(l: &mut Liquid, frame: &Frame, secs: f32, kind: Kind) {
        for _ in 0..(secs * 30.0) as usize {
            l.advance(frame, 40, 30, kind, 1.0 / 30.0);
        }
    }

    /// Music: every band sounds, and the bass beats.
    fn music(i: usize) -> Frame {
        let mut f = Frame { levels: [0.35; BANDS] };
        for b in 0..6 {
            f.levels[b] = if i % 15 < 2 { 0.95 } else { 0.3 };
        }
        f
    }

    #[test]
    fn small_drops_spread_over_the_whole_panel_not_one_blob() {
        let mut l = Liquid::default();
        run(&mut l, &Frame::default(), 2.0, Kind::Soda);
        assert_eq!(l.drops.len(), DROPS);
        // Drops in every part of the panel.
        for (x0, y0) in [(0.0, 0.0), (20.0, 0.0), (0.0, 30.0), (20.0, 30.0)] {
            let n = l.drops.iter().filter(|d| (x0..x0 + 20.0).contains(&d.x) && (y0..y0 + 30.0).contains(&d.y)).count();
            assert!(n >= 6, "drops in each quarter: {n} at {x0},{y0}");
        }
        // Quiet: the drops are apart, with space between them.
        let between = (l.drops[0].x + l.drops[1].x) / 2.0;
        assert!(!l.inside(between, l.drops[0].y), "quiet drops do not touch");
        // The grid gives the same field as asking point by point.
        let g = l.grid(40, 30, 2, 4, 1.0);
        let (i, j) = (37, 61);
        let (x, y) = ((i as f32 + 0.5) * 0.5, (j as f32 + 0.5) * 0.5);
        assert!((g[j * 80 + i] - l.field(x, y, 1.0)).abs() < 1e-3);
    }

    #[test]
    fn each_drop_swells_and_shakes_with_its_own_band() {
        let mut l = Liquid::default();
        run(&mut l, &Frame::default(), 2.0, Kind::Soda);
        let quiet_r = l.drops[40].r;
        // Only band 40 (treble, near the top) sounds.
        let mut f = Frame::default();
        f.levels[40] = 0.9;
        let (x0, y0) = (l.drops[40].x, l.drops[40].y);
        let mut moved = 0.0f32;
        for _ in 0..30 {
            l.advance(&f, 40, 30, Kind::Soda, 1.0 / 30.0);
            moved = moved.max((l.drops[40].x - x0).hypot(l.drops[40].y - y0));
        }
        assert!(l.drops[40].r > quiet_r * 2.0, "it swells: {} from {quiet_r}", l.drops[40].r);
        assert!(moved > 0.3, "it trembles: {moved}");
        assert!((l.drops[3].r - quiet_r).abs() < 0.01, "a silent band's drop stays small");
        assert!(l.drops[40].y < 15.0 && l.drops[3].y > 45.0, "treble at the top, bass at the bottom");
    }

    #[test]
    fn no_gravity_the_drops_stay_spread_with_music_and_calm_in_silence() {
        for (_, kind) in crate::theme::Liquid::ALL {
            let mut l = Liquid::default();
            for i in 0..(20 * 30) {
                l.advance(&music(i), 40, 30, kind, 1.0 / 30.0);
            }
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for d in &l.drops {
                assert!((0.0..=40.0).contains(&d.x) && (0.0..=60.0).contains(&d.y), "{kind:?}: in the panel");
                lo = lo.min(d.y);
                hi = hi.max(d.y);
            }
            assert!(hi - lo > 35.0, "{kind:?}: still spread top to bottom: {lo}..{hi}");
            run(&mut l, &Frame::default(), 5.0, kind);
            let fastest = l.drops.iter().map(|d| d.vx.hypot(d.vy)).fold(0.0, f32::max);
            assert!(fastest < 2.0, "{kind:?}: calm in silence: {fastest}");
        }
    }

    #[test]
    fn a_beat_sends_a_ripple_through_the_drops() {
        let mut l = Liquid::default();
        run(&mut l, &Frame::default(), 2.0, Kind::Ink);
        let before: Vec<(f32, f32)> = l.drops.iter().map(|d| (d.x, d.y)).collect();
        let mut f = Frame::default();
        for b in 0..6 {
            f.levels[b] = 0.95;
        }
        // The bass bands sound too, so the drops of other bands show only the ripple.
        for _ in 0..4 {
            l.advance(&f, 40, 30, Kind::Ink, 1.0 / 30.0);
        }
        let moved = l.drops.iter().zip(&before).skip(12).filter(|(d, (x, y))| (d.x - x).hypot(d.y - y) > 0.3).count();
        assert!(moved > 10, "many drops pushed by the ripple: {moved}");
    }

    #[test]
    fn particles_are_born_and_die() {
        let mut l = Liquid::default();
        let loud = Frame { levels: [0.7; BANDS] };
        let mut most = 0;
        for _ in 0..90 {
            l.advance(&loud, 40, 30, Kind::Soda, 1.0 / 30.0);
            most = most.max(l.particles.iter().filter(|p| p.bit == Bit::Bubble).count());
        }
        assert!(most > 0, "soda fizzes");
        assert!(l.particles.len() <= MAX_PARTICLES);
        run(&mut l, &Frame::default(), 15.0, Kind::Soda);
        assert!(l.particles.is_empty(), "the fizz goes when the music stops");
        let mut ice = Liquid::default();
        run(&mut ice, &Frame::default(), 30.0, Kind::Ice);
        let floes = ice.particles.iter().filter(|p| p.bit == Bit::Floe).count();
        assert!((1..=4).contains(&floes), "ice shards float free: {floes}");
    }
}
