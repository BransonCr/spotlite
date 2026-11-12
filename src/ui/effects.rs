//! The sound visualizer and the side spectrum.

use super::*;

/// The line floats this many quarter-cells above the bar tops.
pub(super) const VIZ_LINE_GAP: i32 = 3;

/// Bar tops in eighths of a cell, for smooth heights.
pub(super) const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Bars reach at most this share of the height.
pub(super) const VIZ_MAX: f32 = 0.55;

/// Level for column `x` of `width`, between the two nearest bands.
pub(super) fn viz_column(frame: &viz::Frame, x: u16, width: u16) -> f32 {
    viz_at(frame, x as f32 / width.saturating_sub(1).max(1) as f32)
}

/// Level at `pos` (0 = left edge, 1 = right edge), with smooth blending between bands.
pub(super) fn viz_at(frame: &viz::Frame, pos: f32) -> f32 {
    frame.at(pos)
}

/// Draws the spectrum behind the UI, rising from the bottom of `area`: smooth bars (eighth
/// blocks) in empty cells only. Text, the selection and pictures are left alone, and so is
/// every cell the bars do not reach, so most frames change one cell per column.
#[cfg(test)]
pub fn draw_visualizer(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    draw_visualizer_fx(buf, area, frame, 1.0);
}

/// The visualizer: `reveal` (0..1) sweeps it in from the left and out again, and a light
/// ripples across the bars.
pub fn draw_visualizer_fx(buf: &mut Buffer, area: Rect, frame: &viz::Frame, reveal: f32) {
    let frame = &revealed(frame, reveal);
    if area.height == 0 || frame.levels.iter().all(|&l| l < 0.01) {
        return;
    }
    match theme::c().viz {
        theme::Viz::None => {}
        theme::Viz::Bars => draw_viz_bars(buf, area, frame),
        theme::Viz::Mirror => draw_viz_mirror(buf, area, frame),
        theme::Viz::Dots => draw_viz_dots(buf, area, frame),
        theme::Viz::Wave => draw_viz_wave(buf, area, frame),
    }
}

/// `frame` as far as the sweep has come: bands come in from the left, each with a short ease.
pub(super) fn revealed(frame: &viz::Frame, reveal: f32) -> viz::Frame {
    if reveal >= 1.0 {
        return *frame;
    }
    let mut out = *frame;
    for (b, l) in out.levels.iter_mut().enumerate() {
        let e = (reveal * 1.6 - b as f32 / (viz::BANDS - 1) as f32 * 0.6).clamp(0.0, 1.0);
        *l *= e * e * (3.0 - 2.0 * e);
    }
    out
}

/// A band of light that travels across the screen (0..1 at `pos`, 0 = left edge).
pub(super) fn viz_ripple(pos: f32) -> f32 {
    let secs = anim(16) as f32 * 0.016;
    0.5 + 0.5 * (pos * 9.0 - secs * 4.5).sin()
}

/// The middle line of the mirror style, and how far its bars reach.
pub(super) fn mirror_axis(area: Rect) -> (u16, f32) {
    (area.y + area.height * 3 / 4, (area.height / 4) as f32 * 0.9)
}

/// Smooth bars with a line over them; the light ripples across their tops.
pub(super) fn draw_viz_wave(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let max = area.height as f32 * VIZ_MAX;
    let shades = theme::viz_shades();
    for x in area.left()..area.right() {
        let height = viz_column(frame, x - area.x, area.width) * max;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        for k in 0..=full.min(area.height - 1) {
            let symbol = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if symbol == " " {
                continue;
            }
            let y = area.bottom() - 1 - k;
            let cell = &mut buf[(x, y)];
            let sym = cell.symbol();
            if cell.skip || cell.bg != Color::Reset || sym != " " {
                continue;
            }
            let shade = shades[(k as usize * shades.len() / area.height.max(1) as usize).min(shades.len() - 1)];
            let pos = (x - area.x) as f32 / area.width.max(2) as f32;
            let glow = 0.3 * viz_ripple(pos) * (k as f32 + 1.0) / (full as f32 + 1.0);
            cell.set_symbol(symbol).set_fg(theme::blend(shade, theme::blend(t.accent, t.accent2, pos), glow));
        }
    }
    draw_viz_line(buf, area, frame);
}

/// A visualizer cell may be drawn only where nothing else is.
pub(super) fn viz_free(buf: &Buffer, x: u16, y: u16) -> bool {
    let cell = &buf[(x, y)];
    !cell.skip && cell.bg == Color::Reset && cell.symbol() == " "
}

/// Separate chunky bars (2 cells wide, 1 apart), shaded upward and tinted across the screen
/// from accent to accent2, with a brighter cap.
pub(super) fn draw_viz_bars(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let shades = theme::viz_shades();
    let max = area.height as f32 * VIZ_MAX;
    let w = area.width.max(2) as f32 - 1.0;
    let mut x = area.x;
    while x + 1 < area.right() {
        let height = viz_column(frame, x + 1 - area.x, area.width) * max;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        let hue = theme::blend(t.accent, t.accent2, (x - area.x) as f32 / w);
        let top = if part > 0 { full } else { full.saturating_sub(1) };
        for k in 0..=full.min(area.height - 1) {
            let symbol = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if symbol == " " {
                continue;
            }
            let y = area.bottom() - 1 - k;
            let shade = shades[(k as usize * shades.len() / area.height.max(1) as usize).min(shades.len() - 1)];
            let ripple = 0.2 * viz_ripple((x - area.x) as f32 / w);
            let colour = theme::blend(shade, hue, if k == top { 0.45 + ripple } else { 0.15 + ripple * 0.6 });
            for dx in 0..2 {
                if viz_free(buf, x + dx, y) {
                    buf[(x + dx, y)].set_symbol(symbol).set_fg(colour);
                }
            }
        }
        x += 3;
    }
}

/// Bars that grow up and down from the middle line, like a sound wave in an editor; the tips
/// glow toward the accent.
pub(super) fn draw_viz_mirror(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let shades = theme::viz_shades();
    // The horizon sits low, in the empty space under the list, and the wave stays small.
    let (mid, half) = mirror_axis(area);
    for x in area.left()..area.right() {
        let height = viz_column(frame, x - area.x, area.width) * half;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        for k in 0..=full {
            let shade = shades[(k as usize * shades.len() / (half as usize + 1)).min(shades.len() - 1)];
            let ripple = 0.25 * viz_ripple((x - area.x) as f32 / area.width.max(2) as f32);
            let colour = if k + 1 >= full { theme::blend(shade, t.accent, 0.12 + ripple) } else { theme::blend(shade, t.accent, ripple * 0.4) };
            // Up: eighth blocks from the bottom of each cell.
            let up = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if up != " " && mid > area.y + k {
                let y = mid - 1 - k;
                if y >= area.y && viz_free(buf, x, y) {
                    buf[(x, y)].set_symbol(up).set_fg(colour);
                }
            }
            // Down: full cells, then an upper half block for the rest.
            let down = if k < full { "█" } else if part >= 4 { "▀" } else { " " };
            let y = mid + k;
            if down != " " && y < area.bottom() && viz_free(buf, x, y) {
                buf[(x, y)].set_symbol(down).set_fg(theme::blend(colour, Color::Rgb(0, 0, 0), 0.25));
            }
        }
    }
}

/// Star dust: a braille dot at each peak, and a few twinkling dots under it.
pub(super) fn draw_viz_dots(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let sub_w = area.width as usize * 2;
    let sub_h = area.height as i32 * 4;
    if sub_w < 2 {
        return;
    }
    let max = area.height as f32 * VIZ_MAX * 4.0;
    let tick = anim(400) as u32;
    let mut dots: std::collections::HashMap<(u16, u16), (u8, bool)> = std::collections::HashMap::new();
    for sx in 0..sub_w {
        let level = viz_at(frame, sx as f32 / (sub_w - 1) as f32);
        let peak = sub_h - 1 - (level * max).round() as i32;
        for sy in peak.max(0)..sub_h {
            // The peak always; below it, about one dot in nine, changing every 0.4 s.
            let h = (sx as u32).wrapping_mul(73_856_093) ^ (sy as u32).wrapping_mul(19_349_663) ^ tick.wrapping_mul(83_492_791);
            let is_peak = sy == peak;
            if !is_peak && !h.is_multiple_of(9) {
                continue;
            }
            let cell = (area.x + (sx / 2) as u16, area.y + (sy / 4) as u16);
            let e = dots.entry(cell).or_insert((0, false));
            e.0 |= BRAILLE[sx % 2][(sy % 4) as usize];
            e.1 |= is_peak;
        }
    }
    for ((x, y), (bits, peak)) in dots {
        if !viz_free(buf, x, y) {
            continue;
        }
        if let Some(c) = char::from_u32(0x2800 + bits as u32) {
            buf[(x, y)].set_char(c).set_fg(if peak { t.viz_line } else { theme::blend(t.viz_high, t.viz_line, 0.4) });
        }
    }
}

/// Braille dot bits for (column 0..2, row 0..4) inside one cell.
pub(super) const BRAILLE: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A thin braille line (2x4 dots per cell) that follows the bar tops, a little above them.
pub(super) fn draw_viz_line(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let sub_w = area.width as usize * 2;
    let sub_h = area.height as i32 * 4;
    if sub_w < 2 {
        return;
    }
    let max = area.height as f32 * VIZ_MAX * 4.0;
    let ys: Vec<i32> = (0..sub_w)
        .map(|sx| sub_h - 1 - (viz_at(frame, sx as f32 / (sub_w - 1) as f32) * max).round() as i32 - VIZ_LINE_GAP)
        .collect();
    let mut dots: std::collections::HashMap<(u16, u16), u8> = std::collections::HashMap::new();
    for sx in 0..sub_w {
        // Not where the bars are not swept in yet.
        if viz_at(frame, sx as f32 / (sub_w - 1) as f32) < 0.002 {
            continue;
        }
        // Join each dot to the midpoint toward its neighbour, so steep parts have no gaps.
        let (y, next) = (ys[sx], ys.get(sx + 1).copied().unwrap_or(ys[sx]));
        let mid = (y + next) / 2;
        for dy in y.min(mid)..=y.max(mid) {
            if !(0..sub_h).contains(&dy) {
                continue;
            }
            let cell = (area.x + (sx / 2) as u16, area.y + (dy / 4) as u16);
            *dots.entry(cell).or_insert(0) |= BRAILLE[sx % 2][(dy % 4) as usize];
        }
    }
    for ((x, y), bits) in dots {
        let cell = &mut buf[(x, y)];
        if cell.skip || cell.bg != Color::Reset || cell.symbol() != " " {
            continue;
        }
        if let Some(c) = char::from_u32(0x2800 + bits as u32) {
            cell.set_char(c).set_fg(theme::c().viz_line);
        }
    }
}

/// Bar ends in eighths of a cell, from the left: for bars that grow to the right.
pub(super) const LEFT_EIGHTHS: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// Side bars reach at most this share of the width.
pub(super) const SIDE_MAX: f32 = 0.66;

/// The side visualizer, over the sidebar's song: the spectrum turned on its side. Bass at the
/// bottom, treble at the top; each band is a bar that grows from the left edge to the right,
/// in empty cells only. A light ripples up the bars, a cap hangs over each bar and falls back
/// with gravity, and sparks fly off the loud bar ends. `reveal` (0..1) sweeps it in from the
/// bottom row up, and out again.
pub fn draw_side_visualizer(buf: &mut Buffer, area: Rect, frame: &viz::Frame, peaks: &viz::Peaks, reveal: f32) {
    if area.height < 2 || area.width < 4 || reveal <= 0.0 {
        return;
    }
    let t = theme::c();
    let secs = anim(16) as f32 * 0.016;
    let h = area.height as f32;
    let maxw = area.width as f32 * SIDE_MAX;
    for k in 0..area.height {
        let y = area.bottom() - 1 - k;
        let pos = k as f32 / (h - 1.0).max(1.0);
        // Rows come in from the bottom up, each with a short ease.
        let e = (reveal * 1.6 - pos * 0.6).clamp(0.0, 1.0);
        let row_in = e * e * (3.0 - 2.0 * e);
        if row_in <= 0.0 {
            continue;
        }
        let level = viz_at(frame, pos) * row_in;
        let len = level * maxw;
        let hue = theme::blend(t.accent, t.accent2, pos);
        // A band of light that travels up the bars.
        let ripple = 0.5 + 0.5 * (pos * 9.0 - secs * 4.5).sin();
        let full = len.floor() as u16;
        let part = ((len - full as f32) * 8.0).round() as usize;
        let cells = if part > 0 { full + 1 } else { full };
        for i in 0..cells.min(area.width) {
            let x = area.x + i;
            if !viz_free(buf, x, y) {
                continue;
            }
            let along = (i as f32 + 0.5) / len.max(1.0);
            let base = theme::blend(t.viz_low, t.viz_high, (i as f32 / maxw.max(1.0)) * 1.4);
            let glow = 0.12 + 0.55 * along.powi(2) + 0.2 * ripple * along;
            let colour = theme::blend(base, hue, glow);
            let symbol = match t.viz {
                // Separate segments, like a level meter.
                theme::Viz::Bars => {
                    if i < full || part >= 4 {
                        "▊"
                    } else {
                        continue;
                    }
                }
                // Dots: braille columns, full or half.
                theme::Viz::Dots => {
                    if i < full || part >= 4 {
                        "⣿"
                    } else {
                        "⡇"
                    }
                }
                _ => {
                    if i < full {
                        "█"
                    } else {
                        LEFT_EIGHTHS[part]
                    }
                }
            };
            buf[(x, y)].set_symbol(symbol).set_fg(colour);
        }
        // The cap: a thin bright line over the bar, falling back when the bar drops.
        let peak = (peak_at(peaks, pos) * row_in * maxw).round() as u16;
        if peak > cells && peak < area.width && viz_free(buf, area.x + peak, y) {
            let cap = theme::blend(t.viz_line, hue, 0.55);
            buf[(area.x + peak, y)].set_symbol("▏").set_fg(cap);
        }
        // Sparks: loud bars throw dots off their ends, which drift right and fade.
        if level > 0.22 {
            let reach = (area.width as f32 - len).min(maxw * 0.6);
            for j in 0..3u32 {
                let seed = (k as u32).wrapping_mul(2_654_435_761) ^ j.wrapping_mul(40_503);
                let offset = (seed % 1000) as f32 / 1000.0;
                let speed = 0.55 + level * 0.9 + (seed >> 10) as f32 % 7.0 * 0.05;
                let phase = (secs * speed + offset).fract();
                // Only some sparks are lit at a time, more when the band is loud.
                if (seed >> 3) % 10 >= (level * 10.0) as u32 + 2 {
                    continue;
                }
                let x = area.x + (len + 1.0 + phase * reach).round() as u16;
                if x >= area.right() || !viz_free(buf, x, y) {
                    continue;
                }
                let symbol = if phase < 0.35 { "•" } else if phase < 0.7 { "∙" } else { "·" };
                buf[(x, y)].set_symbol(symbol).set_fg(theme::blend(hue, t.viz_low, phase));
            }
        }
    }
}

/// Peak level at `pos`, like `viz_at`.
pub(super) fn peak_at(peaks: &viz::Peaks, pos: f32) -> f32 {
    viz_at(&viz::Frame { levels: peaks.level }, pos)
}
