//! The aura of the sidebar effect (V), drawn: light that comes off the cover of the playing
//! song, in the cover's colours, and moves to the music. Each theme has its own form
//! (`theme::Aura`). `crate::aura` keeps what lasts between frames.
//!
//! Every form draws only glyphs, only in the cells nothing else uses: text, the selected row
//! and pictures stay as they are, and the terminal's background shows through. The colours are
//! kept dim and in a few steps, so the light stays behind the text and a cell that does not
//! change is not sent again.

use super::*;
use crate::aura::{Aura, Geo};
use theme::Aura as Kind;

type Rgb = [f32; 3];

fn rgb(c: Color) -> Rgb {
    match c {
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0],
        _ => [0.5; 3],
    }
}

fn mix(a: Rgb, b: Rgb, f: f32) -> Rgb {
    let f = f.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f)
}

/// The steps of light a colour can have: few, so a slow change does not redraw the cell.
const STEPS: f32 = 14.0;

/// `colour` at strength `a` (0..1) over the dark `low`.
fn lit(low: Rgb, colour: Rgb, a: f32) -> Color {
    let a = (a.clamp(0.0, 1.0) * STEPS).round() / STEPS;
    let c = mix(low, colour, a).map(|v| (v * 255.0).round() as u8);
    Color::Rgb(c[0], c[1], c[2])
}

/// A number 0..1 from three, the same each time.
fn hash(a: u32, b: u32, c: u32) -> f32 {
    let h = (a.wrapping_mul(73_856_093) ^ b.wrapping_mul(19_349_663) ^ c.wrapping_mul(83_492_791)).wrapping_mul(2_654_435_761);
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Braille dots over a panel: 2 × 4 to a cell. Each cell takes the colour of its strongest dot.
struct Dots {
    cols: usize,
    bits: Vec<u8>,
    a: Vec<f32>,
    colour: Vec<Rgb>,
}

impl Dots {
    fn new(area: Rect) -> Dots {
        let n = area.width as usize * area.height as usize;
        Dots { cols: area.width as usize, bits: vec![0; n], a: vec![0.0; n], colour: vec![[0.0; 3]; n] }
    }

    /// Lights dot (`sx`, `sy`), counted in dots from the panel's top left.
    fn set(&mut self, sx: usize, sy: usize, a: f32, colour: Rgb) {
        let i = (sy / 4) * self.cols + sx / 2;
        let Some(bits) = self.bits.get_mut(i) else { return };
        if sx / 2 >= self.cols {
            return;
        }
        *bits |= BRAILLE[sx % 2][sy % 4];
        if a > self.a[i] {
            self.a[i] = a;
            self.colour[i] = colour;
        }
    }

    /// Draws the dots, in the cells of `clear` only.
    fn flush(self, buf: &mut Buffer, area: Rect, clear: &[bool], low: Rgb) {
        for (i, &bits) in self.bits.iter().enumerate() {
            let (x, y) = (area.x + (i % self.cols) as u16, area.y + (i / self.cols) as u16);
            if bits == 0 || !clear[i] {
                continue;
            }
            if let Some(c) = char::from_u32(0x2800 + bits as u32) {
                buf[(x, y)].set_char(c).set_fg(lit(low, self.colour[i], self.a[i]));
            }
        }
    }
}

/// What every form needs to draw.
struct Pen<'a> {
    area: Rect,
    geo: Geo,
    aura: &'a Aura,
    frame: &'a viz::Frame,
    kind: Kind,
    t: theme::Theme,
    /// The cells the aura may use, row by row: nothing is in them, and no text is next to
    /// them on their row (so nothing shows between two words).
    clear: Vec<bool>,
    /// The dark end of every colour: near the panel's own background.
    low: Rgb,
    /// How far the effect is in (0..1, eased), and how far from the cover it reaches now: it
    /// grows out of the cover, and goes back into it.
    grow: f32,
    reach: f32,
}

impl Pen<'_> {
    /// 1 where the effect has grown to, 0 beyond, soft between.
    fn gate(&self, d: f32) -> f32 {
        ((self.reach - d) / 3.0).clamp(0.0, 1.0)
    }

    /// The colour of light that comes from (u, v) of the cover: the cover's colour there with
    /// a little of the theme in it, or the theme's own colours while no cover is known.
    fn tone(&self, u: f32, v: f32) -> Rgb {
        let own = mix(rgb(self.t.accent), rgb(self.t.accent2), u * 0.6 + v * 0.4);
        let colour = match self.aura.colour(u, v) {
            Some(c) => mix(own, mix(c, own, 0.15), self.aura.tint),
            None => own,
        };
        match self.kind {
            // A print: almost no colour.
            Kind::Halftone => mix(rgb(self.t.muted), colour, 0.25),
            _ => colour,
        }
    }

    /// 0 in silence, 1 when any music sounds: the forms keep a little light all over the
    /// panel in a quiet part of a song, and go back to the cover only when the music stops.
    fn sound(&self) -> f32 {
        let e = ((self.aura.energy - 0.02) / 0.08).clamp(0.0, 1.0);
        e * e * (3.0 - 2.0 * e)
    }

    fn free(&self, col: u16, row: u16) -> bool {
        self.clear[row as usize * self.area.width as usize + col as usize]
    }

    fn put(&self, buf: &mut Buffer, col: u16, row: u16, symbol: &str, colour: Rgb, a: f32) {
        buf[(self.area.x + col, self.area.y + row)].set_symbol(symbol).set_fg(lit(self.low, colour, a));
    }

    /// Grooves: thin rings round the cover, like the grooves of a record. Each ring bends out
    /// where its band is loud, the rings reach farther when the music is loud, they creep out
    /// slowly, and a beat's pulse lights them as it passes.
    fn grooves(&self, buf: &mut Buffer) {
        let g = self.geo;
        // Bright near the cover; with music the rings go on to the far end of the panel.
        let spread = (1.6 * self.aura.energy).min(0.7).max(0.3 * self.sound());
        let creep = self.aura.time * 0.3;
        let mut dots = Dots::new(self.area);
        for row in 0..self.area.height {
            for col in 0..self.area.width {
                if !self.free(col, row) {
                    continue;
                }
                let (cx, cy) = (col as f32 + 0.5, row as f32 * 2.0 + 1.0);
                let d = g.dist(cx, cy);
                let level = self.frame.at(g.band(cx, cy));
                let a = ((0.42 + 0.5 * level) * (-d / 9.0).exp().max(spread) + 0.4 * self.aura.pulse_at(d)) * self.gate(d);
                if a < 0.06 {
                    continue;
                }
                let (u, v) = g.uv(cx, cy);
                let colour = self.tone(u, v);
                for dc in 0..2 {
                    for dr in 0..4 {
                        let (x, y) = (col as f32 + (dc as f32 + 0.5) / 2.0, row as f32 * 2.0 + (dr as f32 + 0.5) / 2.0);
                        let dd = g.dist(x, y);
                        // Each dot has its own bend, so a ring stays one line from cell to cell.
                        let bend = self.frame.at(g.band(x, y)) * 4.0 * (dd / 3.0).min(1.0);
                        if dd > 0.3 && (dd - bend - creep - 0.8).rem_euclid(3.0) < 0.5 {
                            dots.set(col as usize * 2 + dc, row as usize * 4 + dr, a, colour);
                        }
                    }
                }
            }
        }
        dots.flush(buf, self.area, &self.clear, self.low);
    }

    /// Stardust: the dust that drifts off the cover, each piece in the colour of the place it
    /// left; a few are stars that flash with the treble.
    fn stardust(&self, buf: &mut Buffer) {
        let g = self.geo;
        let treble = self.frame.levels[30..].iter().sum::<f32>() / (viz::BANDS - 30) as f32;
        let mut dots = Dots::new(self.area);
        let mut stars = Vec::new();
        for m in &self.aura.motes {
            // Bright for most of its life, so it still shows at the far end of the panel.
            let fade = (m.age / 0.5).min(1.0) * ((1.0 - m.age / m.life) / 0.25).clamp(0.0, 1.0);
            let a = fade * (0.7 + 0.3 * self.aura.energy + 0.3 * treble) * self.gate(self.geo.dist(m.x, m.y));
            if a < 0.05 {
                continue;
            }
            let colour = self.tone(m.u, m.v);
            if m.star {
                stars.push((m, a, colour));
            } else {
                // The piece, and a short tail behind it.
                let speed = m.vx.hypot(m.vy).max(0.01);
                for (back, share) in [(0.0, 1.0), (0.6, 0.6), (1.2, 0.35)] {
                    let (x, y) = (m.x - m.vx / speed * back, m.y - m.vy / speed * back);
                    if x >= 0.0 && y >= 0.0 && g.dist(x, y) > 0.0 {
                        dots.set((x * 2.0) as usize, (y * 2.0) as usize, a * share, colour);
                    }
                }
            }
        }
        dots.flush(buf, self.area, &self.clear, self.low);
        for (m, a, colour) in stars {
            let (col, row) = (m.x as u16, (m.y / 2.0) as u16);
            if col >= self.area.width || row >= self.area.height {
                continue;
            }
            if !self.free(col, row) {
                continue;
            }
            let flash = treble > 0.25 && (self.aura.time * 6.0 + m.life * 40.0).sin() > 0.0;
            self.put(buf, col, row, if flash { "✦" } else { "·" }, mix(colour, rgb(self.t.bright), 0.3), a + if flash { 0.3 } else { 0.1 });
        }
    }

    /// Pixels: square lights round the cover, each the colour of the cover's edge next to it:
    /// a level meter that goes out from the cover in every direction, with ragged ends that
    /// flicker.
    fn pixels(&self, buf: &mut Buffer) {
        let g = self.geo;
        let tick = (self.aura.time * 5.0) as u32;
        let (far, quiet) = (g.far(), self.sound());
        let first = g.x0 as i32;
        for row in 0..self.area.height {
            for col in 0..self.area.width {
                // A light every second column, in line with the cover's left edge.
                if (col as i32 - first).rem_euclid(2) != 0 || !self.free(col, row) {
                    continue;
                }
                let (cx, cy) = (col as f32 + 0.5, row as f32 * 2.0 + 1.0);
                let d = g.dist(cx, cy);
                let level = self.frame.at(g.band(cx, cy));
                // With loud music the loud bands reach the far end of the panel.
                let reach = (1.6 + level * far * (0.5 + 1.1 * self.aura.energy)).max(far * 0.4 * quiet) + 4.0 * self.aura.pulse_at(d);
                let end = d + hash(col as u32, row as u32, tick) * 2.2;
                if end > reach {
                    continue;
                }
                let tip = if end > reach - 1.2 { 0.14 } else { 0.0 };
                let a = ((0.3 + 0.45 * level) * (1.0 - d / reach).max(0.0).sqrt() + tip) * self.gate(d);
                if a < 0.05 {
                    continue;
                }
                let (u, v) = g.uv(cx, cy);
                self.put(buf, col, row, "■", self.tone(u, v), a);
            }
        }
    }

    /// Aurora: curtains of light that hang from the top of the panel, behind the cover, and
    /// sway. The cover's colours run down them. The hem of the curtains is the spectrum: it
    /// hangs lower where the band is loud.
    fn aurora(&self, buf: &mut Buffer) {
        // Thresholds for the dots of a cell, in an ordered pattern: more dots where it is bright.
        const ORDER: [[u8; 4]; 2] = [[0, 6, 1, 7], [4, 2, 5, 3]];
        let g = self.geo;
        let (flow, quiet) = (self.aura.flow, self.sound());
        let top = (g.y1 + 2.0).min(g.h);
        let (sw, sh) = (self.area.width as usize * 2, self.area.height as usize * 4);
        let sway: Vec<f32> = (0..sh).map(|sy| 1.6 * (sy as f32 * 0.055 + flow * 0.5).sin()).collect();
        let mut dots = Dots::new(self.area);
        for sx in 0..sw {
            let px = (sx as f32 + 0.5) / 2.0;
            let level = self.frame.at(px / g.w);
            // The curtains hang into the lower panel with any music, and to the bottom where a
            // band is loud; the hem waves slowly.
            let least = 0.12 + 0.33 * quiet * (1.0 + 0.25 * (px * 0.35 + flow * 0.4).sin());
            let hem = (top + (g.h - top) * (least + (1.0 - least) * level)) * self.grow;
            let under = g.has_cover() && px >= g.x0 && px < g.x1;
            for (sy, sway) in sway.iter().enumerate() {
                let py = (sy as f32 + 0.5) / 2.0;
                if py > hem {
                    break;
                }
                if under && py < g.y1 {
                    continue;
                }
                let x = px + sway;
                let fold = 0.6 * (0.5 + 0.5 * (x * 0.6 + flow * 0.3).sin()) + 0.4 * (0.5 + 0.5 * (x * 1.7 - flow * 0.45 + 1.7).sin());
                // Bright near the hem, fading up; the hem itself is soft.
                let rise = (py / hem).powf(1.6) * (hem - py).min(1.0);
                let a = fold.powf(1.3) * rise * (0.45 + 0.6 * level) * self.grow;
                if a > 0.05 + (ORDER[sx % 2][sy % 4] as f32 + 0.5) / 20.0 {
                    dots.set(sx, sy, 0.25 + a, self.tone(px / g.w, py / g.h));
                }
            }
        }
        dots.flush(buf, self.area, &self.clear, self.low);
    }

    /// Ribbons: three soft ribbons that flow across the panel, the first from behind the
    /// cover's bottom edge. One for the bass, one for the middle, one for the treble: each
    /// waves higher and grows thicker with its part of the music, and takes its colours from
    /// the cover as it crosses. A faint copy trails each one.
    fn ribbons(&self, buf: &mut Buffer) {
        use std::f32::consts::{PI, TAU};
        let g = self.geo;
        let part = |r: std::ops::Range<usize>| self.frame.levels[r.clone()].iter().sum::<f32>() / r.len() as f32;
        let levels = [part(0..10), part(10..28), part(28..viz::BANDS)];
        let below = (g.y1 + 1.0).min(g.h);
        let span = (g.h - below).max(4.0);
        let (sw, sh) = (self.area.width as usize * 2, self.area.height as i32 * 4);
        let mut dots = Dots::new(self.area);
        for (k, level) in levels.into_iter().enumerate() {
            let base = below + span * [0.06, 0.4, 0.76][k];
            let amp = 1.0 + (level * span * 0.16).min(span * 0.2);
            let f = TAU / g.w * [0.9, 1.3, 1.7][k];
            // The trail first, then the ribbon over it.
            for (lag, share, body) in [(0.55, 0.4, 0.0), (0.0, 1.0, 1.0)] {
                let phase = (self.aura.flow - lag) * [0.8, 1.1, 1.5][k] + k as f32 * 2.1;
                for sx in 0..sw {
                    let px = (sx as f32 + 0.5) / 2.0;
                    let yc = base + amp * (px * f + phase).sin() + 0.4 * amp * (px * f * 2.1 - phase * 1.3 + k as f32).sin();
                    // Thin at both ends.
                    let taper = (PI * px / g.w).sin().max(0.0).powf(0.6);
                    let half = ((0.5 + 2.2 * level * body) * taper / 2.0).max(0.26);
                    let a = (0.36 + 0.5 * level) * share * (0.4 + 0.6 * taper) * self.gate(g.dist(px, yc));
                    if a < 0.05 {
                        continue;
                    }
                    let colour = self.tone(px / g.w, [0.3, 0.6, 0.9][k]);
                    let (lo, hi) = (((yc - half) * 2.0 - 0.5).ceil() as i32, ((yc + half) * 2.0 - 0.5).floor() as i32);
                    for sy in lo.max(0)..=hi.min(sh - 1) {
                        dots.set(sx, sy as usize, a, colour);
                    }
                }
            }
        }
        dots.flush(buf, self.area, &self.clear, self.low);
    }

    /// Traces: wires that run from the cover's edges across the panel, each in the colour of
    /// the place it starts. A wire glows with its band, and pulses of light run down it when
    /// the band jumps.
    fn traces(&self, buf: &mut Buffer) {
        for w in &self.aura.wires {
            let level = self.frame.at(w.band);
            let colour = self.tone(w.u, w.v);
            let shown = (self.grow * w.path.len() as f32).ceil() as usize;
            for (i, &(col, row, glyph)) in w.path.iter().enumerate().take(shown) {
                if col >= self.area.width || row >= self.area.height || !self.free(col, row) {
                    continue;
                }
                // The light of a pulse: full at its head, a tail behind it.
                let glow = w
                    .heads
                    .iter()
                    .map(|h| {
                        let behind = h - i as f32;
                        if behind >= 0.0 { (-behind / 2.5).exp() } else { (1.0 + behind).max(0.0) }
                    })
                    .fold(0.0, f32::max);
                let d = self.geo.dist(col as f32 + 0.5, row as f32 * 2.0 + 1.0);
                let a = 0.13 + 0.3 * level + 0.6 * glow + 0.2 * self.aura.pulse_at(d);
                let glyph = match glyph {
                    '│' if glow > 0.6 => '┃',
                    '─' if glow > 0.6 => '━',
                    c => c,
                };
                let fg = lit(self.low, mix(colour, rgb(self.t.bright), glow * 0.45), a * self.grow);
                buf[(self.area.x + col, self.area.y + row)].set_char(glyph).set_fg(fg);
            }
        }
    }

    /// Halftone: the cover printed on round the picture, in dots: larger where the cover's
    /// edge is light and where the band is loud. It has no motion of its own.
    fn halftone(&self, buf: &mut Buffer) {
        let g = self.geo;
        // With loud music the print covers the whole panel.
        let far = 4.0 + g.far() * (0.1 + 1.8 * self.aura.energy).max(0.55 * self.sound());
        for row in 0..self.area.height {
            for col in 0..self.area.width {
                if (col + row) % 2 != 0 || !self.free(col, row) {
                    continue;
                }
                let (cx, cy) = (col as f32 + 0.5, row as f32 * 2.0 + 1.0);
                let d = g.dist(cx, cy);
                let level = self.frame.at(g.band(cx, cy));
                let (u, v) = g.uv(cx, cy);
                let ink = self.aura.colour(u, v).map_or(0.6, |c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]);
                let a = (ink.powf(0.7) * (1.0 - d / far).max(0.0) * (0.35 + 0.9 * level) + 0.35 * self.aura.pulse_at(d)) * self.gate(d);
                let symbol = match a {
                    a if a < 0.07 => continue,
                    a if a < 0.22 => "·",
                    a if a < 0.42 => "•",
                    _ => "●",
                };
                self.put(buf, col, row, symbol, self.tone(u, v), 0.3 + a);
            }
        }
    }
}

/// The aura of the sidebar effect, in `area`, round `cover` (the cover's place, if it shows).
/// `reveal` (0..1) grows it out of the cover, and takes it back in.
pub fn draw_aura(buf: &mut Buffer, area: Rect, cover: Option<Rect>, aura: &Aura, frame: &viz::Frame, kind: Kind, reveal: f32) {
    let area = area.intersection(buf.area);
    if area.height < 2 || area.width < 4 || reveal <= 0.0 {
        return;
    }
    let t = theme::c();
    let geo = Geo::new(area, cover);
    let grow = reveal * reveal * (3.0 - 2.0 * reveal);
    let text = |x: u16, y: u16| {
        let cell = &buf[(x, y)];
        !cell.skip && cell.symbol() != " "
    };
    let clear = (area.top()..area.bottom())
        .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
        .map(|(x, y)| viz_free(buf, x, y) && !(x > area.left() && text(x - 1, y)) && !(x + 1 < area.right() && text(x + 1, y)))
        .collect();
    let pen = Pen { area, geo, aura, frame, kind, t, clear, low: rgb(t.viz_low), grow, reach: grow * (geo.w.hypot(geo.h) + 3.0) };
    match kind {
        Kind::Grooves => pen.grooves(buf),
        Kind::Stardust => pen.stardust(buf),
        Kind::Pixels => pen.pixels(buf),
        Kind::Aurora => pen.aurora(buf),
        Kind::Ribbons => pen.ribbons(buf),
        Kind::Traces => pen.traces(buf),
        Kind::Halftone => pen.halftone(buf),
    }
}
