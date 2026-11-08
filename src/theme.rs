//! Themes. Each theme is a small commented TOML file: "#rrggbb" colours and a few style
//! characters (see themes/ in the repo). The built-in ones are compiled in;
//! `~/.config/spotlite/themes/<name>.toml` adds a theme, or replaces a built-in one of the same
//! name. `T` in the app picks one; `spotlite theme ...` does the same from a shell (for
//! scripts and AI agents). A running spotlite reloads the theme when its file or the chosen
//! name changes. The background stays the terminal's own, so its opacity shows through.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub text: Color,
    pub bright: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub selected: Color,
    pub accent: Color,
    /// End of the progress-bar gradient (it starts at `accent`).
    pub accent2: Color,
    pub red: Color,
    pub green: Color,
    /// The like heart.
    pub love: Color,
    pub viz_low: Color,
    pub viz_high: Color,
    pub viz_line: Color,
    /// Panel corners: rounded, plain, thick or double.
    pub corners: BorderType,
    /// Before the selected row.
    pub marker: char,
    /// Before "spotlite" and the playing song.
    pub logo: char,
    /// Filled and empty parts of the progress and volume bars.
    pub bar: (char, char),
    /// Separator in "artist · album" and between footer hints.
    pub bullet: char,
    pub playing: Playing,
    /// Animations (playing mark, glow, progress wave) while music plays.
    pub motion: bool,
    pub layout: Layout,
    pub frame: Frame,
    pub frame_fx: FrameFx,
    pub titles: Titles,
    pub header: Header,
    /// Nerd Font glyphs (powerline arrows). Ghostty and kitty have them built in.
    pub nerd: bool,
    pub select: Select,
    pub viz: Viz,
    pub progress: Progress,
    pub volume: Volume,
    /// Dotted leaders from each song title to its time, like a printed track list.
    pub leaders: bool,
}

/// The playing mark: equalizer, spinner, pulse, or the still logo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playing {
    Eq,
    Spin,
    Pulse,
    Logo,
}

/// Where things go: player at the bottom; a big cover column on wide terminals; player on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Classic,
    Stage,
    Top,
}

/// Panel lines: all four sides; only a top rule with the title; none (space only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Full,
    Rule,
    None,
}

/// Colour of the panel lines: flat; a gradient from accent to accent2; a slow glow while playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFx {
    Plain,
    Gradient,
    Glow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Titles {
    Normal,
    Lower,
    Upper,
    Spaced,
}

/// The tab bar: underlined; pills; [ brackets ]; powerline segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    Plain,
    Pills,
    Brackets,
    Powerline,
}

/// The selected row: marker + soft background; inverse block; underline; arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    Bar,
    Block,
    Underline,
    Arrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viz {
    Wave,
    Bars,
    Mirror,
    Dots,
    None,
}

/// Progress bar: a line; a line with a knob; the `bar` characters; a moving wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Line,
    Knob,
    Blocks,
    Wave,
}

/// Volume: a bar, or LED segments coloured like a VU meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Volume {
    Bar,
    Leds,
}

/// Every value of a style key, with its name in the theme file.
fn pick<T: Copy>(key: &str, s: &str, options: &[(&str, T)]) -> Result<T, String> {
    options.iter().find(|(n, _)| *n == s).map(|(_, v)| *v).ok_or_else(|| {
        let names: Vec<&str> = options.iter().map(|(n, _)| *n).collect();
        format!("{key}: “{s}” is not one of: {}", names.join(", "))
    })
}

pub const DEFAULT: &str = "amber";

/// Built-in themes, in picker order.
pub const BUILTIN: &[(&str, &str)] = &[
    ("amber", include_str!("../themes/amber.toml")),
    ("rose-pine", include_str!("../themes/rose-pine.toml")),
    ("gruvbox", include_str!("../themes/gruvbox.toml")),
    ("nord", include_str!("../themes/nord.toml")),
    ("catppuccin", include_str!("../themes/catppuccin.toml")),
    ("tokyo-night", include_str!("../themes/tokyo-night.toml")),
    ("mono", include_str!("../themes/mono.toml")),
];

const AMBER: Theme = Theme {
    text: Color::Rgb(0xd9, 0xd8, 0xd2),
    bright: Color::Rgb(0xec, 0xeb, 0xe6),
    muted: Color::Rgb(0x88, 0x87, 0x80),
    faint: Color::Rgb(0x5f, 0x5e, 0x59),
    border: Color::Rgb(0x2e, 0x2e, 0x31),
    selected: Color::Rgb(0x23, 0x23, 0x26),
    accent: Color::Rgb(0xe3, 0xa5, 0x48),
    accent2: Color::Rgb(0xd9, 0x67, 0x5e),
    red: Color::Rgb(0xd9, 0x67, 0x5e),
    green: Color::Rgb(0x94, 0xb4, 0x79),
    love: Color::Rgb(0xff, 0x5f, 0x7e),
    viz_low: Color::Rgb(0x0b, 0x10, 0x16),
    viz_high: Color::Rgb(0x1b, 0x27, 0x36),
    viz_line: Color::Rgb(0x4d, 0x61, 0x76),
    corners: BorderType::Rounded,
    marker: '▌',
    logo: '♪',
    bar: ('━', '─'),
    bullet: '·',
    playing: Playing::Eq,
    motion: true,
    layout: Layout::Classic,
    frame: Frame::Full,
    frame_fx: FrameFx::Plain,
    titles: Titles::Normal,
    header: Header::Plain,
    nerd: true,
    select: Select::Bar,
    viz: Viz::Wave,
    progress: Progress::Line,
    volume: Volume::Bar,
    leaders: false,
};

#[cfg(not(test))]
static CURRENT: RwLock<(Theme, String)> = RwLock::new((AMBER, String::new()));

// Tests run in parallel: each test thread has its own theme, so one test's theme does not
// change what another draws.
#[cfg(test)]
thread_local! {
    static CURRENT: RwLock<(Theme, String)> = const { RwLock::new((AMBER, String::new())) };
}

fn with<R>(f: impl FnOnce(&RwLock<(Theme, String)>) -> R) -> R {
    #[cfg(not(test))]
    return f(&CURRENT);
    #[cfg(test)]
    return CURRENT.with(f);
}

/// The theme in use.
pub fn c() -> Theme {
    with(|c| c.read().map(|t| t.0).unwrap_or(AMBER))
}

/// Name of the theme in use.
pub fn name() -> String {
    let n = with(|c| c.read().map(|t| t.1.clone()).unwrap_or_default());
    if n.is_empty() { DEFAULT.to_owned() } else { n }
}

pub fn set(name: &str, theme: Theme) {
    with(|c| {
        if let Ok(mut t) = c.write() {
            *t = (theme, name.to_owned());
        }
    });
}

/// Uses the named theme. On an error (no such theme, a bad value) nothing changes.
pub fn apply(name: &str) -> Result<(), String> {
    set(name, load(name)?);
    Ok(())
}

/// Reads the named theme, without using it.
pub fn load(name: &str) -> Result<Theme, String> {
    let (_, text) = all().into_iter().find(|(n, _)| n == name).ok_or_else(|| format!("no theme “{name}”"))?;
    parse(&text).map_err(|e| format!("theme “{name}”: {e}"))
}

/// The file of a theme in the themes folder (it may not exist: built-ins live in the binary).
pub fn file(name: &str) -> PathBuf {
    dir().join(format!("{name}.toml"))
}

pub fn dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("spotlite/themes")
}

/// Every theme: built-ins first, then files from the themes folder (which replace a built-in
/// of the same name). Read fresh each call.
pub fn all() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILTIN.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect();
    let mut extra = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir()) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            let (Some(name), Ok(text)) = (path.file_stem().and_then(|s| s.to_str()), std::fs::read_to_string(&path)) else {
                continue;
            };
            match out.iter_mut().find(|(n, _)| n == name) {
                Some(slot) => slot.1 = text,
                None => extra.push((name.to_owned(), text)),
            }
        }
    }
    extra.sort();
    out.extend(extra);
    out
}

/// Reads a theme file. Missing keys take the amber value; an unknown key or a bad value is an
/// error (so a typo does not pass quietly).
pub fn parse(text: &str) -> Result<Theme, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.message().trim().to_owned())?;
    let mut t = AMBER;
    for (key, value) in &table {
        if let Some(b) = value.as_bool() {
            match key.as_str() {
                "motion" => t.motion = b,
                "nerd" => t.nerd = b,
                "leaders" => t.leaders = b,
                _ => return Err(format!("{key}: write the value in quotes")),
            }
            continue;
        }
        if matches!(key.as_str(), "motion" | "nerd" | "leaders") {
            return Err(format!("{key}: write true or false (no quotes)"));
        }
        let s = value.as_str().ok_or_else(|| format!("{key}: write the value in quotes"))?;
        let chars: Vec<char> = s.chars().collect();
        let narrow = |c: &char| unicode_width::UnicodeWidthChar::width(*c) == Some(1);
        match key.as_str() {
            "corners" => {
                t.corners = match s {
                    "rounded" => BorderType::Rounded,
                    "plain" => BorderType::Plain,
                    "thick" => BorderType::Thick,
                    "double" => BorderType::Double,
                    _ => return Err(format!("corners: “{s}” is not rounded, plain, thick or double")),
                };
                continue;
            }
            "playing" => {
                t.playing = pick(key, s, &[("eq", Playing::Eq), ("spin", Playing::Spin), ("pulse", Playing::Pulse), ("logo", Playing::Logo)])?;
                continue;
            }
            "layout" => {
                t.layout = pick(key, s, &[("classic", Layout::Classic), ("stage", Layout::Stage), ("top", Layout::Top)])?;
                continue;
            }
            "frame" => {
                t.frame = pick(key, s, &[("full", Frame::Full), ("rule", Frame::Rule), ("none", Frame::None)])?;
                continue;
            }
            "frame_fx" => {
                t.frame_fx = pick(key, s, &[("plain", FrameFx::Plain), ("gradient", FrameFx::Gradient), ("glow", FrameFx::Glow)])?;
                continue;
            }
            "titles" => {
                t.titles = pick(
                    key,
                    s,
                    &[("normal", Titles::Normal), ("lower", Titles::Lower), ("upper", Titles::Upper), ("spaced", Titles::Spaced)],
                )?;
                continue;
            }
            "header" => {
                t.header = pick(
                    key,
                    s,
                    &[("plain", Header::Plain), ("pills", Header::Pills), ("brackets", Header::Brackets), ("powerline", Header::Powerline)],
                )?;
                continue;
            }
            "select" => {
                t.select = pick(
                    key,
                    s,
                    &[("bar", Select::Bar), ("block", Select::Block), ("underline", Select::Underline), ("arrow", Select::Arrow)],
                )?;
                continue;
            }
            "viz" => {
                t.viz = pick(
                    key,
                    s,
                    &[("wave", Viz::Wave), ("bars", Viz::Bars), ("mirror", Viz::Mirror), ("dots", Viz::Dots), ("none", Viz::None)],
                )?;
                continue;
            }
            "progress" => {
                t.progress = pick(
                    key,
                    s,
                    &[("line", Progress::Line), ("knob", Progress::Knob), ("blocks", Progress::Blocks), ("wave", Progress::Wave)],
                )?;
                continue;
            }
            "volume" => {
                t.volume = pick(key, s, &[("bar", Volume::Bar), ("leds", Volume::Leds)])?;
                continue;
            }
            "marker" | "logo" | "bullet" => {
                let [c] = chars[..] else { return Err(format!("{key}: give one character")) };
                if !narrow(&c) {
                    return Err(format!("{key}: “{c}” is not one cell wide"));
                }
                match key.as_str() {
                    "marker" => t.marker = c,
                    "logo" => t.logo = c,
                    _ => t.bullet = c,
                }
                continue;
            }
            "bar" => {
                let [a, b] = chars[..] else { return Err("bar: give two characters, filled then empty".into()) };
                if !narrow(&a) || !narrow(&b) {
                    return Err("bar: each character must be one cell wide".into());
                }
                t.bar = (a, b);
                continue;
            }
            _ => {}
        }
        let slot = match key.as_str() {
            "text" => &mut t.text,
            "bright" => &mut t.bright,
            "muted" => &mut t.muted,
            "faint" => &mut t.faint,
            "border" => &mut t.border,
            "selected" => &mut t.selected,
            "accent" => &mut t.accent,
            "accent2" => &mut t.accent2,
            "red" => &mut t.red,
            "green" => &mut t.green,
            "love" => &mut t.love,
            "viz_low" => &mut t.viz_low,
            "viz_high" => &mut t.viz_high,
            "viz_line" => &mut t.viz_line,
            _ => return Err(format!("unknown key “{key}”")),
        };
        *slot = hex(s).ok_or_else(|| format!("{key}: “{s}” is not a \"#rrggbb\" colour"))?;
    }
    Ok(t)
}

fn hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// A panel or section title in the theme's letter case: "Liked songs", "liked songs",
/// "LIKED SONGS" or "L I K E D  S O N G S".
pub fn title(s: &str) -> String {
    match c().titles {
        Titles::Normal => s.to_owned(),
        Titles::Lower => s.to_lowercase(),
        Titles::Upper => s.to_uppercase(),
        Titles::Spaced => s
            .to_uppercase()
            .split(' ')
            .map(|w| w.chars().map(String::from).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("  "),
    }
}

/// The theme named by SPOTLITE_THEME, for this run only.
pub fn env_override() -> Option<String> {
    std::env::var("SPOTLITE_THEME").ok().filter(|s| !s.is_empty())
}

/// Animations are on: the theme allows them and SPOTLITE_NO_MOTION is not set.
pub fn motion() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    c().motion && !*OFF.get_or_init(|| std::env::var_os("SPOTLITE_NO_MOTION").is_some())
}

/// The mark of the playing song: animated while it plays, the logo when paused.
pub fn playing_mark(playing: bool, frame: usize) -> char {
    let t = c();
    if !playing || !motion() {
        return t.logo;
    }
    let frames: &[char] = match t.playing {
        Playing::Eq => &['▁', '▃', '▅', '▇', '▅', '▃'],
        Playing::Spin => &['◐', '◓', '◑', '◒'],
        Playing::Pulse => &['·', '•', '●', '•'],
        Playing::Logo => return t.logo,
    };
    frames[frame % frames.len()]
}

/// The colour `f` (0 to 1) of the way from `a` to `b`.
pub fn blend(a: Color, b: Color, f: f32) -> Color {
    let (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) = (a, b) else { return a };
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(mix(r0, r1), mix(g0, g1), mix(b0, b1))
}

/// Five visualizer shades from `viz_low` to `viz_high`, bottom to top.
pub fn viz_shades() -> [Color; 5] {
    let t = c();
    std::array::from_fn(|i| blend(t.viz_low, t.viz_high, i as f32 / 4.0))
}

/// Quiet lines that are not panel frames (empty bar parts, separators): between the frame
/// colour and `faint`, so frame effects leave them alone.
pub fn dim() -> Color {
    let t = c();
    blend(t.border, t.faint, 0.3)
}

/// The like heart, filled or outline: Nerd Font hearts fill the cell (ghostty and kitty have
/// them built in); `nerd = false` uses ♥ / ♡.
pub fn heart(filled: bool) -> &'static str {
    match (c().nerd, filled) {
        (true, true) => "\u{f004}",
        (true, false) => "\u{f08a}",
        (false, true) => "♥",
        (false, false) => "♡",
    }
}

/// The mark before the selected row, by `select` style.
pub fn marker() -> String {
    let t = c();
    match t.select {
        Select::Bar | Select::Underline => t.marker.to_string(),
        Select::Block => " ".into(),
        Select::Arrow => "❯".into(),
    }
}

pub fn text() -> Style {
    Style::new().fg(c().text)
}

pub fn bright() -> Style {
    Style::new().fg(c().bright).add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::new().fg(c().muted)
}

pub fn faint() -> Style {
    Style::new().fg(c().faint)
}

pub fn accent() -> Style {
    Style::new().fg(c().accent)
}

pub fn border(focused: bool) -> Style {
    Style::new().fg(if focused { c().accent } else { c().border })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_amber_is_the_fallback() {
        for (name, text) in BUILTIN {
            assert!(parse(text).is_ok(), "{name}");
        }
        let amber = parse(BUILTIN[0].1).unwrap();
        assert_eq!((amber.accent, amber.text, amber.border), (AMBER.accent, AMBER.text, AMBER.border), "amber.toml has the fallback colours");
        let looks: std::collections::HashSet<String> = BUILTIN
            .iter()
            .map(|(_, text)| {
                let t = parse(text).unwrap();
                format!("{:?}{:?}{:?}{:?}{:?}", t.layout, t.frame, t.header, t.viz, t.select)
            })
            .collect();
        assert_eq!(looks.len(), BUILTIN.len(), "every theme has its own look, not only its own colours");
        let t = parse("accent = \"#ff0000\"").unwrap();
        assert_eq!((t.accent, t.text), (Color::Rgb(255, 0, 0), AMBER.text));
        assert!(parse("accent = \"red\"").is_err());
        assert!(parse("acent = \"#ff0000\"").is_err(), "a typo is reported");
        let t = parse("corners = \"double\"\nmarker = \"┃\"\nbar = \"▰▱\"").unwrap();
        assert_eq!((t.corners, t.marker, t.bar), (BorderType::Double, '┃', ('▰', '▱')));
        assert!(parse("bar = \"█\"").is_err(), "the bar needs two characters");
        assert!(parse("logo = \"音\"").is_err(), "a wide character would break the layout");
        assert!(parse("corners = \"wavy\"").is_err());
        let t = parse("layout = \"stage\"\nframe = \"rule\"\nframe_fx = \"glow\"\ntitles = \"spaced\"\nheader = \"powerline\"\nselect = \"arrow\"\nviz = \"mirror\"\nprogress = \"wave\"\nvolume = \"leds\"\nplaying = \"spin\"\nmotion = false\nnerd = true\nleaders = true").unwrap();
        assert_eq!((t.layout, t.frame, t.frame_fx, t.titles), (Layout::Stage, Frame::Rule, FrameFx::Glow, Titles::Spaced));
        assert_eq!((t.header, t.select, t.viz, t.progress), (Header::Powerline, Select::Arrow, Viz::Mirror, Progress::Wave));
        assert_eq!((t.volume, t.playing, t.motion, t.nerd, t.leaders), (Volume::Leds, Playing::Spin, false, true, true));
        let e = parse("viz = \"lasers\"").unwrap_err();
        assert!(e.contains("wave, bars, mirror, dots, none"), "the error lists the choices: {e}");
        assert!(parse("motion = \"yes\"").is_err(), "a switch is true or false");
        set("x", Theme { titles: Titles::Spaced, ..AMBER });
        assert_eq!(title("Liked songs"), "L I K E D  S O N G S");
        set(DEFAULT, AMBER);
        let shades = {
            set("x", Theme { viz_low: Color::Rgb(0, 0, 0), viz_high: Color::Rgb(40, 80, 120), ..AMBER });
            viz_shades()
        };
        assert_eq!(shades[0], Color::Rgb(0, 0, 0));
        assert_eq!(shades[2], Color::Rgb(20, 40, 60));
        assert_eq!(shades[4], Color::Rgb(40, 80, 120));
        set(DEFAULT, AMBER);
    }
}
