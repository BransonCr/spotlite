//! The share card (`C`, or "Share card" in the sidebar): a 1200x630 PNG of the playing song,
//! with its cover, title, artist and album on a background in the cover's own colour. It is
//! saved to the Pictures folder (spotlite) and its path is copied, ready to paste.

use crate::api::{pick_image, Track};
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use anyhow::{anyhow, Context, Result};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use std::path::PathBuf;

const W: u32 = 1200;
const H: u32 = 630;
const COVER: u32 = 470;
const PAD: u32 = 80;
const TEXT_X: f32 = (PAD + COVER + 60) as f32;
const TEXT_W: f32 = W as f32 - TEXT_X - PAD as f32;

/// Makes the card for `track`, saves it, and copies its path. Returns the path.
pub fn make(track: &Track) -> Result<PathBuf> {
    let url = pick_image(track.images(), 600).ok_or_else(|| anyhow!("This song has no cover."))?;
    let bytes = crate::images::cover_bytes(url).ok_or_else(|| anyhow!("Could not download the cover."))?;
    let cover = image::load_from_memory(&bytes).context("reading the cover")?.to_rgba8();
    let bold = font(true)?;
    let regular = font(false)?;
    let album = track.album.as_ref().map(|a| match a.year() {
        Some(y) => format!("{}  ·  {y}", a.name),
        None => a.name.clone(),
    });
    let card = compose(&cover, &track.name, &track.artist_names(), album.as_deref(), &bold, &regular);
    let dir = dirs::picture_dir().or_else(dirs::home_dir).ok_or_else(|| anyhow!("No Pictures folder."))?.join("spotlite");
    std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
    let path = dir.join(format!("{}.png", file_name(&format!("{} - {}", track.artist_names(), track.name))));
    card.save(&path).with_context(|| format!("saving {}", path.display()))?;
    // Copy the path, so it can be pasted into a chat at once.
    crate::platform::copy_text(&path.display().to_string());
    Ok(path)
}

/// The share card's font: a sans-serif the system has (see `platform::font_file`).
fn font(bold: bool) -> Result<FontVec> {
    let file = crate::platform::font_file(bold)?;
    let data = std::fs::read(&file).with_context(|| format!("reading the font {}", file.display()))?;
    FontVec::try_from_vec_and_index(data, 0).map_err(|_| anyhow!("cannot read the font {}", file.display()))
}

/// Letters that are safe in a file name; others become spaces.
fn file_name(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c.is_alphanumeric() || " -_.,'()&".contains(c) { c } else { ' ' }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean.chars().take(120).collect();
    if clean.is_empty() { "song".into() } else { clean }
}

/// The cover's average colour.
fn average(img: &RgbaImage) -> [f32; 3] {
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for p in img.pixels().step_by(7) {
        for c in 0..3 {
            sum[c] += p[c] as f64;
        }
        n += 1.0;
    }
    let n = n.max(1.0);
    let avg = [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32];
    // An average of many colours is grey; push it away from grey so the card has a colour.
    let grey = (avg[0] + avg[1] + avg[2]) / 3.0;
    avg.map(|c| (grey + (c - grey) * 1.8).clamp(0.0, 255.0))
}

pub fn compose(cover: &RgbaImage, title: &str, artist: &str, album: Option<&str>, bold: &FontVec, regular: &FontVec) -> RgbaImage {
    let avg = average(cover);
    // Background: the cover's colour, dark, a little darker at the bottom.
    let mut img = RgbaImage::from_fn(W, H, |_, y| {
        let f = 0.34 - 0.12 * (y as f32 / H as f32);
        Rgba([(avg[0] * f) as u8, (avg[1] * f) as u8, (avg[2] * f) as u8, 255])
    });
    // The cover with rounded corners.
    let small = image::imageops::resize(cover, COVER, COVER, FilterType::Lanczos3);
    let r = 18.0f32;
    for (x, y, p) in small.enumerate_pixels() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let cx = fx.clamp(r, COVER as f32 - r);
        let cy = fy.clamp(r, COVER as f32 - r);
        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
        let a = (r - d + 0.5).clamp(0.0, 1.0);
        if a > 0.0 {
            blend(&mut img, (PAD + x) as i32, (PAD + y) as i32, [p[0], p[1], p[2]], a);
        }
    }
    let light = [
        (avg[0] * 0.4 + 255.0 * 0.6) as u8,
        (avg[1] * 0.4 + 255.0 * 0.6) as u8,
        (avg[2] * 0.4 + 255.0 * 0.6) as u8,
    ];
    let white = [255, 255, 255];
    // Text, centred on the cover's height.
    let title_lines = wrap(title, 2, |s| width(bold, 58.0, s));
    struct TextLine<'a> {
        font: &'a FontVec,
        size: f32,
        text: String,
        color: [u8; 3],
        alpha: f32,
        /// Down to the next line.
        step: f32,
    }
    let line = |font, size, text, color, alpha, step| TextLine { font, size, text, color, alpha, step };
    let mut lines = vec![line(regular, 22.0, "NOW PLAYING".into(), light, 1.0, 40.0)];
    for l in title_lines {
        lines.push(line(bold, 58.0, l, white, 1.0, 66.0));
    }
    lines.push(line(regular, 32.0, fit(artist, |s| width(regular, 32.0, s)), white, 0.9, 52.0));
    if let Some(a) = album {
        lines.push(line(regular, 25.0, fit(a, |s| width(regular, 25.0, s)), white, 0.6, 38.0));
    }
    let total: f32 = lines.iter().map(|l| l.step).sum();
    let mut y = (PAD + COVER / 2) as f32 - total / 2.0;
    for l in &lines {
        draw_text(&mut img, l.font, l.size, TEXT_X, y + l.size, &l.text, l.color, l.alpha);
        y += l.step;
    }
    draw_text(&mut img, regular, 22.0, TEXT_X, (H - PAD + 10) as f32, "spotlite", light, 0.7);
    img
}

fn width(font: &FontVec, size: f32, s: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// `s` cut to fit TEXT_W, ending with "…" when cut.
fn fit(s: &str, measure: impl Fn(&str) -> f32) -> String {
    if measure(s) <= TEXT_W {
        return s.to_owned();
    }
    let mut out: String = s.to_owned();
    while !out.is_empty() && measure(&format!("{out}…")) > TEXT_W {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

/// Words in lines that fit TEXT_W, at most `max` lines; the last one ends with "…" if cut.
fn wrap(s: &str, max: usize, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let words: Vec<&str> = s.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let next = if cur.is_empty() { (*w).to_owned() } else { format!("{cur} {w}") };
        if measure(&next) <= TEXT_W || cur.is_empty() {
            cur = next;
            continue;
        }
        lines.push(std::mem::take(&mut cur));
        if lines.len() == max {
            let rest = words[i..].join(" ");
            let last = lines.pop().unwrap_or_default();
            lines.push(fit(&format!("{last} {rest}"), &measure));
            return lines;
        }
        cur = (*w).to_owned();
    }
    if !cur.is_empty() {
        lines.push(fit(&cur, &measure));
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn draw_text(img: &mut RgbaImage, font: &FontVec, size: f32, x: f32, baseline: f32, s: &str, color: [u8; 3], alpha: f32) {
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    let mut caret = x;
    let mut prev = None;
    for c in s.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, baseline));
        caret += sf.h_advance(id);
        prev = Some(id);
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                blend(img, b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, color, cov * alpha);
            });
        }
    }
}

fn blend(img: &mut RgbaImage, x: i32, y: i32, color: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    let a = a.clamp(0.0, 1.0);
    for c in 0..3 {
        p[c] = (p[c] as f32 * (1.0 - a) + color[c] as f32 * a).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit per character: easy to reason about.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32 * (TEXT_W / 10.0)
    }

    #[test]
    fn wrap_keeps_two_lines_and_cuts_the_rest() {
        assert_eq!(wrap("short", 2, chars), ["short"]);
        assert_eq!(wrap("aaaa bbbb cccc", 2, chars), ["aaaa bbbb", "cccc"]);
        let lines = wrap("aaaa bbbb cccc dddd eeee", 2, chars);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{lines:?}");
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("AC/DC - Back: In Black?"), "AC DC - Back In Black");
        assert_eq!(file_name("///"), "song");
    }

    #[test]
    fn composes_a_card_when_a_font_is_there() {
        let (Ok(bold), Ok(regular)) = (font(true), font(false)) else { return };
        let cover = RgbaImage::from_pixel(64, 64, Rgba([200, 40, 40, 255]));
        let card = compose(&cover, "A long song title that wraps", "Artist", Some("Album  ·  2020"), &bold, &regular);
        assert_eq!(card.dimensions(), (W, H));
        // The background takes the cover's colour (red, dark).
        let bg = card.get_pixel(W - 5, 5);
        assert!(bg[0] > bg[1] && bg[0] > bg[2], "{bg:?}");
    }
}

