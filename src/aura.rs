//! The aura of the sidebar effect (V): light that comes off the cover of the playing song.
//!
//! The cover is the source. Its colours (8 × 8 points of the picture) are the colours of the
//! light, and the light starts at its edges. The music moves it: each band of the sound
//! visualizer has a direction from the cover (bass down, treble up the sides), and a bass beat
//! sends a pulse out. Each theme shows the light in its own form (`theme::Aura`); the forms are
//! drawn in `ui::aura`.
//!
//! This file holds what must be kept between frames: the time, the beat, the cover's colours
//! (they fade from one cover to the next), and the small parts some forms have (dust, wires).
//! It is cheap: no fixed step, at most 110 pieces of dust or 3 pulses on each wire, and it runs
//! only while the effect shows.
//!
//! Space is in cell widths; a cell is about twice as tall as wide, so y is in half rows and
//! shapes look round on screen.

use crate::images::{Images, Swatch, SWATCH};
use crate::theme::Aura as Kind;
use crate::viz::{Frame, BANDS};
use ratatui::layout::Rect;
use std::time::Instant;

/// A pulse lives this long, and runs out from the cover this fast (cell widths per second).
pub const PULSE_LIFE: f32 = 1.4;
pub const PULSE_SPEED: f32 = 16.0;
const MAX_PULSES: usize = 4;
const MAX_MOTES: usize = 110;
/// Pulses of light on one wire at a time.
const MAX_HEADS: usize = 3;

/// The panel and the cover in it, in cell widths (x) and half rows (y) from the panel's top left.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Geo {
    pub w: f32,
    pub h: f32,
    /// The cover's sides. Without a cover: a point at the middle of the top edge.
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Geo {
    pub fn new(area: Rect, cover: Option<Rect>) -> Geo {
        let (w, h) = (area.width as f32, area.height as f32 * 2.0);
        match cover.map(|c| c.intersection(area)).filter(|c| !c.is_empty()) {
            Some(c) => {
                let (x0, y0) = ((c.x - area.x) as f32, (c.y - area.y) as f32 * 2.0);
                Geo { w, h, x0, x1: x0 + c.width as f32, y0, y1: y0 + c.height as f32 * 2.0 }
            }
            None => Geo { w, h, x0: w / 2.0, x1: w / 2.0, y0: 0.0, y1: 0.0 },
        }
    }

    pub fn has_cover(&self) -> bool {
        self.x1 > self.x0
    }

    /// How far (x, y) is from the cover (0 inside it).
    pub fn dist(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x0 - x).max(x - self.x1).max(0.0);
        let dy = (self.y0 - y).max(y - self.y1).max(0.0);
        dx.hypot(dy)
    }

    /// The point of the cover nearest to (x, y), as shares of its width and height: the place
    /// the light at (x, y) comes from. Without a cover, the place in the panel.
    pub fn uv(&self, x: f32, y: f32) -> (f32, f32) {
        if !self.has_cover() {
            return ((x / self.w.max(1.0)).clamp(0.0, 1.0), (y / self.h.max(1.0)).clamp(0.0, 1.0));
        }
        (((x - self.x0) / (self.x1 - self.x0)).clamp(0.0, 1.0), ((y - self.y0) / (self.y1 - self.y0).max(1.0)).clamp(0.0, 1.0))
    }

    /// The band of the direction from the cover's middle to (x, y), as a place in the spectrum
    /// (0 = lowest): bass straight down, higher bands to both sides, treble up.
    pub fn band(&self, x: f32, y: f32) -> f32 {
        let (cx, cy) = ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0);
        ((x - cx).atan2(y - cy).abs() / (std::f32::consts::PI * 0.62)).min(1.0)
    }
}

/// A pulse from a bass beat: a ring that runs out from the cover.
#[derive(Debug, Clone, Copy)]
pub struct Pulse {
    pub age: f32,
    pub strength: f32,
}

/// A piece of dust (stardust).
#[derive(Debug, Clone, Copy)]
pub struct Mote {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub age: f32,
    pub life: f32,
    /// The place of the cover it left (its colour).
    pub u: f32,
    pub v: f32,
    /// A few are stars: a glyph, not a dot.
    pub star: bool,
}

/// A wire from the cover (traces).
#[derive(Debug, Clone)]
pub struct Wire {
    /// Its cells from the cover out: column, row (from the panel's top left) and line glyph.
    pub path: Vec<(u16, u16, char)>,
    /// Its band, as a place in the spectrum.
    pub band: f32,
    /// The place of the cover it starts at (its colour).
    pub u: f32,
    pub v: f32,
    /// The pulses of light on it: how many cells each is from the cover.
    pub heads: Vec<f32>,
    /// Seconds until the next pulse may start, and a slow copy of its band's level.
    wait: f32,
    slow: f32,
}

#[derive(Debug, Clone)]
pub struct Aura {
    /// Seconds of motion, and a clock that runs faster when the music is loud.
    pub time: f32,
    pub flow: f32,
    /// The mean level of all bands, eased.
    pub energy: f32,
    pub pulses: Vec<Pulse>,
    pub motes: Vec<Mote>,
    pub wires: Vec<Wire>,
    /// The cover's colours now (0..1 each), where they go to, and how much of them shows
    /// (0 = none known: the theme's colours instead).
    colours: [[f32; 3]; SWATCH * SWATCH],
    target: [[f32; 3]; SWATCH * SWATCH],
    pub tint: f32,
    /// The cover the colours are of, and whether they are in.
    cover: Option<String>,
    loaded: bool,
    geo: Geo,
    kind: Option<Kind>,
    /// Slow average of the bass, to find beats over it, and seconds until the next may count.
    bass_avg: f32,
    beat_wait: f32,
    /// Dust owed, a fraction carried between steps.
    owed: f32,
    seed: u32,
    at: Option<Instant>,
}

impl Default for Aura {
    fn default() -> Self {
        Aura {
            time: 0.0,
            flow: 0.0,
            energy: 0.0,
            pulses: Vec::new(),
            motes: Vec::new(),
            wires: Vec::new(),
            colours: [[0.0; 3]; SWATCH * SWATCH],
            target: [[0.0; 3]; SWATCH * SWATCH],
            tint: 0.0,
            cover: None,
            loaded: false,
            geo: Geo::default(),
            kind: None,
            bass_avg: 0.0,
            beat_wait: 0.0,
            owed: 0.0,
            seed: 0x9e37_79b9,
            at: None,
        }
    }
}

/// A cover's colour made fit for light on a dark panel: dark colours are raised and all are a
/// little more vivid, so a dark or grey cover still gives a light that shows.
fn vivid(c: [u8; 3]) -> [f32; 3] {
    let c = c.map(|v| v as f32 / 255.0);
    let grey = (c[0] + c[1] + c[2]) / 3.0;
    let c = c.map(|v| (grey + (v - grey) * 1.35).clamp(0.0, 1.0));
    let top = c[0].max(c[1]).max(c[2]);
    if top < 0.06 {
        return [0.42; 3];
    }
    let gain = (0.72 / top).clamp(1.0, 4.0);
    c.map(|v| (v * gain).min(1.0))
}

impl Aura {
    /// Tells the aura which cover shows (its URL, or None). Its colours are taken when the
    /// picture is loaded.
    pub fn see_cover(&mut self, url: Option<&str>, images: &Images) {
        if self.cover.as_deref() != url {
            self.cover = url.map(str::to_owned);
            self.loaded = false;
        }
        if !self.loaded {
            if let Some(swatch) = url.and_then(|u| images.swatch(u)) {
                self.set_colours(&swatch);
            }
        }
    }

    pub fn set_colours(&mut self, swatch: &Swatch) {
        for (t, c) in self.target.iter_mut().zip(swatch) {
            *t = vivid(*c);
        }
        // The first cover has nothing to fade from.
        if self.tint == 0.0 {
            self.colours = self.target;
        }
        self.loaded = true;
    }

    /// The cover's colour at (u, v) (shares of its width and height), or None when no cover's
    /// colours are known.
    pub fn colour(&self, u: f32, v: f32) -> Option<[f32; 3]> {
        if self.tint <= 0.0 {
            return None;
        }
        let at = |f: f32| ((f.clamp(0.0, 1.0) * SWATCH as f32) as usize).min(SWATCH - 1);
        Some(self.colours[at(v) * SWATCH + at(u)])
    }

    /// Moves the aura on to now. With `motion` off nothing moves in time; the forms still
    /// follow the levels of the music.
    pub fn update(&mut self, frame: &Frame, geo: Geo, kind: Kind, motion: bool) {
        let now = Instant::now();
        let dt = self.at.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.at = Some(now);
        self.advance(frame, geo, kind, if motion { dt } else { 0.0 });
    }

    /// Moves the aura on by `dt` seconds.
    pub fn advance(&mut self, frame: &Frame, geo: Geo, kind: Kind, dt: f32) {
        if self.geo != geo || self.kind != Some(kind) {
            self.geo = geo;
            self.kind = Some(kind);
            self.motes.clear();
            self.wires = if kind == Kind::Traces { wires(geo) } else { Vec::new() };
        }
        let mean = frame.levels.iter().sum::<f32>() / BANDS as f32;
        if dt <= 0.0 {
            // No motion: the levels and the colours as they are, nothing else.
            self.energy = mean;
            if self.loaded {
                self.colours = self.target;
                self.tint = 1.0;
            }
            return;
        }
        self.energy += (mean - self.energy) * (dt * 6.0).min(1.0);
        self.time += dt;
        self.flow += dt * (0.4 + 1.8 * self.energy);
        // The colours fade to those of the new cover (or to the theme's, when there is none).
        let k = (dt * 2.5).min(1.0);
        for (c, t) in self.colours.iter_mut().zip(&self.target) {
            for i in 0..3 {
                c[i] += (t[i] - c[i]) * k;
            }
        }
        self.tint = (self.tint + if self.loaded { dt * 1.5 } else { -dt * 1.5 }).clamp(0.0, 1.0);
        // A bass beat: a pulse runs out from the cover.
        let bass = frame.levels[..6].iter().sum::<f32>() / 6.0;
        self.bass_avg += (bass - self.bass_avg) * (dt * 1.5).min(1.0);
        self.beat_wait -= dt;
        let jump = bass - self.bass_avg;
        let mut beat = 0.0;
        if jump > 0.07 && bass > 0.2 && self.beat_wait <= 0.0 {
            beat = (jump * 4.0).min(1.0);
            self.beat_wait = 0.28;
            if self.pulses.len() == MAX_PULSES {
                self.pulses.remove(0);
            }
            self.pulses.push(Pulse { age: 0.0, strength: beat });
        }
        for p in &mut self.pulses {
            p.age += dt;
        }
        self.pulses.retain(|p| p.age < PULSE_LIFE);
        match kind {
            Kind::Stardust => self.dust(beat, dt),
            Kind::Traces => self.signals(frame, beat, dt),
            _ => {}
        }
    }

    /// How much the pulses light a point `d` from the cover (0..1).
    pub fn pulse_at(&self, d: f32) -> f32 {
        self.pulses
            .iter()
            .map(|p| {
                let off = (d - p.age * PULSE_SPEED) / 1.8;
                p.strength * (1.0 - p.age / PULSE_LIFE) * (-off * off).exp()
            })
            .fold(0.0, f32::max)
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

    /// Stardust: dust leaves the cover's left, bottom and right edges (the top edge is the
    /// panel's top), more of it when the music is loud, and drifts out; a beat pushes it.
    fn dust(&mut self, beat: f32, dt: f32) {
        let g = self.geo;
        self.owed += (4.0 + 44.0 * self.energy) * dt;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            if self.motes.len() >= MAX_MOTES {
                continue;
            }
            // The edges with room for dust next to them: left, bottom, right.
            let room = |free: f32, len: f32| if free >= 2.0 { len } else { 0.0 };
            let (left, bottom, right) = (room(g.x0, g.y1 - g.y0), room(g.h - g.y1, g.x1 - g.x0), room(g.w - g.x1, g.y1 - g.y0));
            let at = self.random() * (left + bottom + right);
            // The place on the edge, and the way out from it.
            let (x, y, nx, ny) = if !g.has_cover() {
                let a = (self.random() - 0.5) * 2.4;
                (g.x0, 0.0, a.sin(), a.cos())
            } else if left + bottom + right <= 0.0 {
                continue;
            } else if at < left {
                (g.x0 - 0.01, g.y0 + at, -1.0, 0.0)
            } else if at < left + bottom {
                (g.x0 + at - left, g.y1, 0.0, 1.0)
            } else {
                (g.x1, g.y0 + at - left - bottom, 1.0, 0.0)
            };
            let speed = 0.9 + self.random() * 1.6;
            let side = (self.random() - 0.5) * 1.6;
            let (u, v) = g.uv(x, y);
            let life = 3.0 + self.random() * 4.0;
            let star = self.random() < 0.1;
            self.motes.push(Mote { x, y, vx: nx * speed - ny * side, vy: ny * speed + nx * side, age: 0.0, life, u, v, star });
        }
        let t = self.time;
        for m in &mut self.motes {
            m.age += dt;
            if beat > 0.0 {
                m.vx *= 1.0 + beat;
                m.vy *= 1.0 + beat;
            }
            // A slow turn, so the paths curve, and the air slows what a beat pushed.
            m.vx += (t * 0.7 + m.y * 0.21).sin() * 0.7 * dt;
            m.vy += (t * 0.5 + m.x * 0.17).cos() * 0.5 * dt;
            let speed = m.vx.hypot(m.vy);
            if speed > 2.2 {
                let slow = 1.0 - (1.2 * dt).min(1.0) * (1.0 - 2.2 / speed);
                m.vx *= slow;
                m.vy *= slow;
            }
            m.x += m.vx * dt;
            m.y += m.vy * dt;
        }
        self.motes.retain(|m| m.age < m.life && m.x >= 0.0 && m.x < g.w && m.y >= 0.0 && m.y < g.h);
    }

    /// Traces: a pulse of light starts down a wire when its band jumps, or on a beat.
    fn signals(&mut self, frame: &Frame, beat: f32, dt: f32) {
        let speed = 14.0 + 12.0 * self.energy;
        for i in 0..self.wires.len() {
            let gap = 0.3 + self.random() * 0.35;
            let w = &mut self.wires[i];
            let level = frame.at(w.band);
            w.wait -= dt;
            let jumped = level - w.slow > 0.05 && level > 0.22;
            if (jumped || (beat > 0.0 && level > 0.15)) && w.wait <= 0.0 && w.heads.len() < MAX_HEADS {
                w.heads.push(0.0);
                w.wait = gap;
            }
            w.slow += (level - w.slow) * (dt * 4.0).min(1.0);
            let end = w.path.len() as f32 + 8.0;
            for h in &mut w.heads {
                *h += speed * dt;
            }
            w.heads.retain(|h| *h < end);
        }
    }
}

/// The wires of `traces`: from the cover's left and right edges out and down the panel's sides,
/// and from its bottom edge down the panel with a few steps to the side. No wire crosses another.
fn wires(g: Geo) -> Vec<Wire> {
    let (cols, rows) = (g.w as i32, (g.h / 2.0) as i32);
    if cols < 4 || rows < 2 {
        return Vec::new();
    }
    let mut used = vec![false; (cols * rows) as usize];
    let (cx0, cx1, cy0, cy1) = (g.x0 as i32, g.x1 as i32, (g.y0 / 2.0) as i32, (g.y1 / 2.0) as i32);
    if g.has_cover() {
        for y in cy0..cy1.min(rows) {
            for x in cx0..cx1.min(cols) {
                used[(y * cols + x) as usize] = true;
            }
        }
    }
    let hash = |a: i32, b: i32| ((a as u32).wrapping_mul(73_856_093) ^ (b as u32).wrapping_mul(19_349_663)).wrapping_mul(2_654_435_761) >> 8;
    let mut out: Vec<Wire> = Vec::new();
    let mut add = |path: Vec<(u16, u16, char)>, used: &mut Vec<bool>| {
        let Some(&(x, y, _)) = path.first() else { return };
        for &(x, y, _) in &path {
            used[(y as i32 * cols + x as i32) as usize] = true;
        }
        let (px, py) = (x as f32 + 0.5, y as f32 * 2.0 + 1.0);
        let (u, v) = g.uv(px, py);
        // The band of the wire's start, so wires next to each other have bands next to each other.
        let band = if g.has_cover() { g.band(px, py) } else { px / g.w };
        out.push(Wire { path, band, u, v, heads: Vec::new(), wait: 0.0, slow: 0.0 });
    };
    // Side wires: out from the cover's edge, then down. The lane farther out starts higher, so
    // its first part does not cross the lanes nearer the cover.
    if g.has_cover() {
        for (dir, edge) in [(-1, cx0 - 1), (1, cx1)] {
            for lane in 0..3 {
                let x_lane = edge + dir * (1 + 2 * lane);
                let y_start = cy1 - 2 - 3 * lane;
                if x_lane < 0 || x_lane >= cols || y_start < cy0 {
                    break;
                }
                let mut path = Vec::new();
                let mut x = edge;
                while x != x_lane {
                    path.push((x as u16, y_start as u16, '─'));
                    x += dir;
                }
                path.push((x_lane as u16, y_start as u16, if dir < 0 { '╭' } else { '╮' }));
                path.extend((y_start + 1..rows).map(|y| (x_lane as u16, y as u16, '│')));
                add(path, &mut used);
            }
        }
    }
    // Bottom wires: down from the cover's bottom edge (from the panel's top without a cover).
    let (from, to, y_top) = if g.has_cover() { (cx0 + 1, cx1 - 1, cy1) } else { (1, cols - 1, 0) };
    let mut x_start = from;
    while x_start < to && y_top < rows {
        let free = |x: i32, y: i32, used: &Vec<bool>| x >= 0 && x < cols && y < rows && !used[(y * cols + x) as usize];
        let (mut x, mut y, mut straight) = (x_start, y_top, 0);
        let mut path = Vec::new();
        while free(x, y, &used) {
            let h = hash(x_start * 31 + y, 7);
            let step = (2 + (h >> 4) % 3) as i32 * if (h >> 8).is_multiple_of(2) { 1 } else { -1 };
            let (lo, hi) = (x.min(x + step), x.max(x + step));
            // A step to the side: now and then, after a straight part, where the way is free.
            let turn = straight >= 2 && y + 2 < rows && h.is_multiple_of(5) && (lo..=hi).all(|i| free(i, y, &used) && free(i, y + 1, &used));
            if turn {
                let dir = step.signum();
                path.push((x as u16, y as u16, if dir > 0 { '╰' } else { '╯' }));
                let mut i = x + dir;
                while i != x + step {
                    path.push((i as u16, y as u16, '─'));
                    i += dir;
                }
                x += step;
                path.push((x as u16, y as u16, if dir > 0 { '╮' } else { '╭' }));
                straight = 0;
            } else {
                path.push((x as u16, y as u16, '│'));
                straight += 1;
            }
            // Mark as it goes, so it does not step over itself.
            for &(px, py, _) in path.iter().rev().take(8) {
                used[(py as i32 * cols + px as i32) as usize] = true;
            }
            y += 1;
        }
        if path.len() >= 2 {
            add(path, &mut used);
        }
        x_start += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 2, y: 5, width: 36, height: 30 };
    const COVER: Rect = Rect { x: 6, y: 5, width: 28, height: 14 };

    fn geo() -> Geo {
        Geo::new(AREA, Some(COVER))
    }

    /// Music: every band sounds, and the bass beats.
    fn music(i: usize) -> Frame {
        let mut f = Frame { levels: [0.35; BANDS] };
        for b in 0..6 {
            f.levels[b] = if i % 15 < 2 { 0.95 } else { 0.3 };
        }
        f
    }

    fn run(a: &mut Aura, kind: Kind, secs: f32, sound: impl Fn(usize) -> Frame) {
        for i in 0..(secs * 30.0) as usize {
            a.advance(&sound(i), geo(), kind, 1.0 / 30.0);
        }
    }

    #[test]
    fn the_cover_is_the_source() {
        let g = geo();
        assert_eq!((g.x0, g.x1, g.y0, g.y1), (4.0, 32.0, 0.0, 28.0));
        assert_eq!(g.dist(10.0, 10.0), 0.0, "inside the cover");
        assert_eq!(g.dist(2.0, 10.0), 2.0, "left of it");
        assert_eq!(g.dist(35.0, 32.0), 5.0, "off its corner");
        assert_eq!(g.uv(2.0, 14.0), (0.0, 0.5), "light on the left comes from the left edge");
        assert_eq!(g.uv(18.0, 50.0), (0.5, 1.0), "light below comes from the bottom edge");
        assert!(g.band(18.0, 55.0) < 0.05, "bass straight down");
        assert!(g.band(1.0, 5.0) > 0.7 && g.band(35.0, 5.0) > 0.7, "treble up both sides");
        let none = Geo::new(AREA, None);
        assert!(!none.has_cover());
        assert_eq!(none.dist(18.0, 6.0), 6.0, "without a cover: from the top's middle");
    }

    #[test]
    fn colours_come_from_the_cover_and_fade_to_the_next() {
        let mut a = Aura::default();
        assert!(a.colour(0.5, 0.5).is_none(), "no cover known: the theme's colours");
        let mut red = [[200u8, 20, 20]; SWATCH * SWATCH];
        red[SWATCH * SWATCH - 1] = [4, 4, 4];
        a.set_colours(&red);
        run(&mut a, Kind::Grooves, 1.0, |_| Frame::default());
        let c = a.colour(0.1, 0.1).unwrap();
        assert!(c[0] > 0.6 && c[1] < 0.2, "red: {c:?}");
        let dark = a.colour(1.0, 1.0).unwrap();
        assert!(dark[0] > 0.3, "black is raised so it shows: {dark:?}");
        a.set_colours(&[[20u8, 20, 200]; SWATCH * SWATCH]);
        a.advance(&Frame::default(), geo(), Kind::Grooves, 0.1);
        let mid = a.colour(0.1, 0.1).unwrap();
        assert!(mid[0] > 0.3 && mid[2] > 0.05, "between the two covers: {mid:?}");
        run(&mut a, Kind::Grooves, 3.0, |_| Frame::default());
        assert!(a.colour(0.1, 0.1).unwrap()[2] > 0.6, "then the new one");
    }

    #[test]
    fn a_bass_beat_sends_a_pulse_out() {
        let mut a = Aura::default();
        run(&mut a, Kind::Grooves, 1.0, |_| Frame::default());
        assert!(a.pulses.is_empty() && a.pulse_at(3.0) == 0.0);
        let mut loud = Frame::default();
        loud.levels[..6].fill(0.95);
        a.advance(&loud, geo(), Kind::Grooves, 1.0 / 30.0);
        assert_eq!(a.pulses.len(), 1);
        run(&mut a, Kind::Grooves, 0.25, |_| loud);
        let d = a.pulses[0].age * PULSE_SPEED;
        assert!(a.pulse_at(d) > 0.3 && a.pulse_at(d + 8.0) < 0.01, "a ring at {d}");
        run(&mut a, Kind::Grooves, 2.0, |_| Frame::default());
        assert!(a.pulses.is_empty(), "it dies");
        run(&mut a, Kind::Grooves, 20.0, music);
        assert!(a.pulses.len() <= MAX_PULSES && a.energy > 0.3 && a.flow > 0.0);
    }

    #[test]
    fn dust_leaves_the_cover_with_the_music() {
        let mut a = Aura::default();
        run(&mut a, Kind::Stardust, 6.0, |_| Frame::default());
        let quiet = a.motes.len();
        assert!((1..40).contains(&quiet), "a little dust in silence: {quiet}");
        run(&mut a, Kind::Stardust, 6.0, music);
        assert!(a.motes.len() > quiet + 10 && a.motes.len() <= MAX_MOTES, "more with music: {}", a.motes.len());
        let g = geo();
        assert!(a.motes.iter().all(|m| m.x >= 0.0 && m.x < g.w && m.y >= 0.0 && m.y < g.h));
        assert!(a.motes.iter().any(|m| g.dist(m.x, m.y) > 4.0), "it drifts away from the cover");
        a.advance(&Frame::default(), g, Kind::Grooves, 0.03);
        assert!(a.motes.is_empty(), "another form has no dust");
    }

    #[test]
    fn wires_run_from_the_cover_and_never_cross() {
        for g in [geo(), Geo::new(AREA, None)] {
            let ws = wires(g);
            assert!(ws.len() >= 8, "wires: {}", ws.len());
            let mut seen = std::collections::HashSet::new();
            for w in &ws {
                let (x, y, _) = w.path[0];
                assert!(!g.has_cover() || g.dist(x as f32 + 0.5, y as f32 * 2.0 + 1.0) <= 1.5, "starts at the cover: {x},{y}");
                for pair in w.path.windows(2) {
                    let (dx, dy) = (pair[1].0 as i32 - pair[0].0 as i32, pair[1].1 as i32 - pair[0].1 as i32);
                    assert_eq!(dx.abs() + dy.abs(), 1, "each cell is next to the last");
                }
                for &(x, y, _) in &w.path {
                    assert!(x < 36 && y < 30, "in the panel");
                    assert!(seen.insert((x, y)), "no cell has two wires: {x},{y}");
                    assert!(!g.has_cover() || g.dist(x as f32 + 0.5, y as f32 * 2.0 + 1.0) > 0.0, "not under the cover");
                }
            }
            assert!(ws.iter().any(|w| w.path.iter().any(|c| c.2 == '─')), "some turn");
            assert!(ws.iter().any(|w| w.path.last().unwrap().1 == 29), "they reach the bottom");
        }
    }

    #[test]
    fn pulses_of_light_run_down_the_wires_with_the_music() {
        let mut a = Aura::default();
        run(&mut a, Kind::Traces, 2.0, |_| Frame::default());
        assert!(!a.wires.is_empty() && a.wires.iter().all(|w| w.heads.is_empty()), "dark in silence");
        run(&mut a, Kind::Traces, 3.0, music);
        let lit = a.wires.iter().filter(|w| !w.heads.is_empty()).count();
        assert!(lit >= 2, "wires with light: {lit}");
        assert!(a.wires.iter().all(|w| w.heads.len() <= MAX_HEADS));
        run(&mut a, Kind::Traces, 5.0, |_| Frame::default());
        assert!(a.wires.iter().all(|w| w.heads.is_empty()), "the light runs off the ends");
    }

    #[test]
    fn without_motion_nothing_moves_in_time() {
        let mut a = Aura::default();
        a.set_colours(&[[200u8, 20, 20]; SWATCH * SWATCH]);
        for i in 0..60 {
            a.advance(&music(i), geo(), Kind::Stardust, 0.0);
        }
        assert_eq!((a.time, a.flow), (0.0, 0.0));
        assert!(a.motes.is_empty() && a.pulses.is_empty());
        assert!(a.energy > 0.3, "it still hears the music");
        assert!(a.colour(0.5, 0.5).is_some(), "and has the cover's colours");
    }
}
