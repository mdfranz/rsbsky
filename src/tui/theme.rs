//! Colour themes (port of Go `internal/tui/theme.go`) and the `themes` preview.
//!
//! Differences: Go asks the terminal for its background colour (OSC 11) to resolve
//! "adaptive" colours; here the background is read from `COLORFGBG`, with no terminal query.
//! When that is not set the background is unknown, and `auto` uses colours readable on both
//! white and black; the other adaptive theme (`catppuccin`) assumes dark.
//! `NO_COLOR` (non-empty) removes every colour; bold/italic/reverse remain.

use std::io::Write;

use ratatui::style::{Color, Modifier, Style};
use thiserror::Error;

/// Theme names in preview/help order.
pub const NAMES: [&str; 6] = ["auto", "dark", "light", "ansi", "dracula", "catppuccin"];

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown theme {0:?} (choose from: {list})", list = NAMES.join(", "))]
pub struct UnknownTheme(pub String);

/// Finds a theme by name, ignoring case; returns its canonical name.
pub fn by_name(name: &str) -> Result<&'static str, UnknownTheme> {
    let lower = name.to_lowercase();
    NAMES
        .iter()
        .copied()
        .find(|n| *n == lower)
        .ok_or_else(|| UnknownTheme(name.to_string()))
}

/// Picks the theme from the `--theme` flag, else the environment, else `auto`.
pub fn resolve(flag: Option<&str>, env: &str) -> Result<&'static str, UnknownTheme> {
    match flag.filter(|f| !f.is_empty()) {
        Some(f) => by_name(f),
        None if !env.is_empty() => by_name(env),
        None => by_name("auto"),
    }
}

/// The colour roles the UI draws with. `None` means "terminal default".
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub text: Option<Color>,
    pub dim: Option<Color>,
    pub warn: Option<Color>,
    pub error: Option<Color>,
    pub ok: Option<Color>,
    pub heading: Option<Color>,
    pub accent: Option<Color>,
    /// Selected-row background; `None` selects with reverse video.
    pub sel_bg: Option<Color>,
    /// Fade ramp for freshly inserted posts, brightest first.
    pub fresh: Vec<Color>,
    /// False under NO_COLOR: every colour above is cleared but the fade ramp keeps its length.
    pub color: bool,
}

fn idx(values: &[u8]) -> Vec<Color> {
    values.iter().map(|v| Color::Indexed(*v)).collect()
}

fn hex(s: &str) -> Color {
    let n = u32::from_str_radix(s.trim_start_matches('#'), 16).expect("hex colour");
    Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}

fn pick(dark_bg: bool, dark: &str, light: &str) -> Option<Color> {
    Some(hex(if dark_bg { dark } else { light }))
}

const DARK_FRESH: [u8; 9] = [213, 177, 141, 105, 69, 61, 60, 59, 238];
const LIGHT_FRESH: [u8; 9] = [201, 165, 129, 93, 63, 103, 145, 188, 252];
/// Fades towards the neutral dim gray rather than into a background we cannot see.
const NEUTRAL_FRESH: [u8; 9] = [201, 165, 129, 93, 99, 105, 103, 102, 243];

/// What is known about the terminal's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
    Unknown,
}

/// Fixed 256-colour indices, not the 16 palette colours: those are whatever the terminal's
/// palette says, and in many light palettes colour 8 (dim) is near white, which made unselected
/// posts invisible. Each passes 4.5:1 against white.
fn light_theme(name: &'static str) -> Theme {
    Theme {
        name,
        text: None,
        dim: Some(Color::Indexed(242)),
        warn: Some(Color::Indexed(130)),
        error: Some(Color::Indexed(160)),
        ok: Some(Color::Indexed(28)),
        heading: Some(Color::Indexed(24)),
        accent: Some(Color::Indexed(127)),
        sel_bg: Some(Color::Indexed(254)),
        fresh: idx(&LIGHT_FRESH),
        color: true,
    }
}

/// For an unknown background: fixed colours that read at 3.9:1 or better on both white and
/// black (about 4.6:1 is the most any colour can manage on both; red tops out just under 4:1,
/// with 196), and reverse video for the selection, which follows the terminal's own colours.
fn neutral_theme(name: &'static str) -> Theme {
    Theme {
        name,
        text: None,
        dim: Some(Color::Indexed(243)),
        warn: Some(Color::Indexed(130)),
        error: Some(Color::Indexed(196)),
        ok: Some(Color::Indexed(28)),
        heading: Some(Color::Indexed(30)),
        accent: Some(Color::Indexed(163)),
        sel_bg: None,
        fresh: idx(&NEUTRAL_FRESH),
        color: true,
    }
}

fn ansi_theme(name: &'static str, sel_bg: Option<Color>, fresh: Vec<Color>) -> Theme {
    Theme {
        name,
        text: None,
        dim: Some(Color::Indexed(8)),
        warn: Some(Color::Indexed(3)),
        error: Some(Color::Indexed(1)),
        ok: Some(Color::Indexed(2)),
        heading: Some(Color::Indexed(6)),
        accent: Some(Color::Indexed(5)),
        sel_bg,
        fresh,
        color: true,
    }
}

impl Theme {
    /// Builds a named theme for a known background. `dark_bg` resolves adaptive colours;
    /// `color` false (NO_COLOR) strips them all.
    pub fn named(name: &str, dark_bg: bool, color: bool) -> Theme {
        let bg = if dark_bg {
            Background::Dark
        } else {
            Background::Light
        };
        Theme::for_background(name, bg, color)
    }

    /// Builds a named theme. Only `auto` treats an unknown background differently; the
    /// other adaptive theme assumes dark.
    pub fn for_background(name: &str, bg: Background, color: bool) -> Theme {
        let name = by_name(name).unwrap_or("auto");
        let dark_bg = bg != Background::Light;
        let mut t = match name {
            "dark" => ansi_theme("dark", Some(Color::Indexed(236)), idx(&DARK_FRESH)),
            "light" => light_theme("light"),
            "ansi" => ansi_theme("ansi", None, idx(&[13, 5, 8])),
            "dracula" => Theme {
                name: "dracula",
                text: Some(hex("#f8f8f2")),
                dim: Some(hex("#6272a4")),
                warn: Some(hex("#f1fa8c")),
                error: Some(hex("#ff5555")),
                ok: Some(hex("#50fa7b")),
                heading: Some(hex("#8be9fd")),
                accent: Some(hex("#ff79c6")),
                sel_bg: Some(hex("#44475a")),
                fresh: ["#ff79c6", "#bd93f9", "#6272a4", "#44475a"]
                    .map(hex)
                    .to_vec(),
                color: true,
            },
            "catppuccin" => Theme {
                name: "catppuccin",
                text: pick(dark_bg, "#cdd6f4", "#4c4f69"),
                dim: pick(dark_bg, "#6c7086", "#9ca0b0"),
                warn: pick(dark_bg, "#f9e2af", "#df8e1d"),
                error: pick(dark_bg, "#f38ba8", "#d20f39"),
                ok: pick(dark_bg, "#a6e3a1", "#40a02b"),
                heading: pick(dark_bg, "#89dceb", "#04a5e5"),
                accent: pick(dark_bg, "#cba6f7", "#8839ef"),
                sel_bg: pick(dark_bg, "#313244", "#ccd0da"),
                fresh: [
                    ("#f5c2e7", "#ea76cb"),
                    ("#cba6f7", "#8839ef"),
                    ("#6c7086", "#9ca0b0"),
                    ("#313244", "#ccd0da"),
                ]
                .iter()
                .map(|(d, l)| hex(if dark_bg { d } else { l }))
                .collect(),
                color: true,
            },
            _ => {
                // auto: the dark theme on a dark background, the light one on a light one,
                // and colours that work on both when we cannot tell
                match bg {
                    Background::Dark => {
                        ansi_theme("auto", Some(Color::Indexed(236)), idx(&DARK_FRESH))
                    }
                    Background::Light => light_theme("auto"),
                    Background::Unknown => neutral_theme("auto"),
                }
            }
        };
        if !color {
            t.text = None;
            t.dim = None;
            t.warn = None;
            t.error = None;
            t.ok = None;
            t.heading = None;
            t.accent = None;
            t.sel_bg = None;
            t.color = false;
        }
        t
    }

    fn fg(c: Option<Color>) -> Style {
        c.map_or_else(Style::new, |c| Style::new().fg(c))
    }

    pub fn base(&self) -> Style {
        Self::fg(self.text)
    }
    pub fn title(&self) -> Style {
        self.base().add_modifier(Modifier::BOLD)
    }
    pub fn dim_style(&self) -> Style {
        Self::fg(self.dim)
    }
    pub fn warn_style(&self) -> Style {
        Self::fg(self.warn)
    }
    pub fn err_style(&self) -> Style {
        Self::fg(self.error)
    }
    pub fn ok_style(&self) -> Style {
        Self::fg(self.ok)
    }
    pub fn heading_style(&self) -> Style {
        Self::fg(self.heading).add_modifier(Modifier::BOLD)
    }
    pub fn accent_style(&self) -> Style {
        Self::fg(self.accent)
    }
    /// Background of the selected row (reverse video when the theme has none).
    pub fn sel_style(&self) -> Style {
        match self.sel_bg {
            Some(bg) => self.base().bg(bg),
            None => self.base().add_modifier(Modifier::REVERSED),
        }
    }
}

/// Whether colour output is wanted (`NO_COLOR` set and non-empty turns it off).
pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// The terminal's background from `COLORFGBG` ("fg;bg", or "fg;other;bg"), if it says.
pub fn background() -> Background {
    parse_colorfgbg(std::env::var("COLORFGBG").ok().as_deref())
}

fn parse_colorfgbg(value: Option<&str>) -> Background {
    match value.and_then(|v| v.rsplit(';').next()?.trim().parse::<u8>().ok()) {
        Some(bg) if bg < 7 || bg == 8 => Background::Dark,
        Some(_) => Background::Light,
        None => Background::Unknown,
    }
}

/// The active theme for `name`, honouring `NO_COLOR` and what is known of the background.
pub fn active(name: &str) -> Theme {
    Theme::for_background(name, background(), color_enabled())
}

fn sgr(style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if m.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if m.contains(Modifier::REVERSED) {
        codes.push("7".into());
    }
    let color = |c: Color, bg: bool| -> Option<String> {
        let base = if bg { 48 } else { 38 };
        match c {
            Color::Indexed(n) => Some(format!("{base};5;{n}")),
            Color::Rgb(r, g, b) => Some(format!("{base};2;{r};{g};{b}")),
            _ => None,
        }
    };
    codes.extend(style.fg.and_then(|c| color(c, false)));
    codes.extend(style.bg.and_then(|c| color(c, true)));
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

fn paint(style: Style, text: &str, ansi: bool) -> String {
    let start = if ansi { sgr(style) } else { String::new() };
    if start.is_empty() {
        text.to_string()
    } else {
        format!("{start}{text}\x1b[0m")
    }
}

/// Previews every theme (the `themes` command). Colour is used only when `ansi` is true.
pub fn print_themes(out: &mut dyn Write, ansi: bool) -> std::io::Result<()> {
    let color = color_enabled();
    let bg = background();
    for name in NAMES {
        let t = Theme::for_background(name, bg, color);
        let ramp: String = t
            .fresh
            .iter()
            .map(|c| {
                paint(
                    Style::new().fg(*c).add_modifier(Modifier::BOLD),
                    "┃",
                    ansi && t.color,
                )
            })
            .collect();
        write!(
            out,
            "{}\n  {} {} {}\n{}\n  {}  {}  {}  fresh {}\n\n",
            t.name,
            paint(t.dim_style(), "4m", ansi),
            paint(t.title(), "Sample post", ansi),
            paint(t.accent_style(), "@you.bsky.social", ansi),
            paint(t.sel_style(), "▸ 4m Sample post @you.bsky.social", ansi),
            paint(t.warn_style(), "status", ansi),
            paint(t.err_style(), "error", ansi),
            paint(t.ok_style(), "kept", ansi),
            ramp
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The xterm RGB value of a 256-colour index from the fixed range (16..=255).
    fn xterm_rgb(n: u8) -> (u8, u8, u8) {
        assert!(n >= 16, "colours 0-15 depend on the terminal's palette");
        if n >= 232 {
            let v = 8 + 10 * (n - 232);
            return (v, v, v);
        }
        let level = |c: u8| if c == 0 { 0 } else { 55 + 40 * c };
        let i = n - 16;
        (level(i / 36), level(i / 6 % 6), level(i % 6))
    }

    fn luminance(n: u8) -> f64 {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = xterm_rgb(n);
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// WCAG contrast ratio against white.
    fn contrast_on_white(n: u8) -> f64 {
        1.05 / (luminance(n) + 0.05)
    }

    /// WCAG contrast ratio against black.
    fn contrast_on_black(n: u8) -> f64 {
        (luminance(n) + 0.05) / 0.05
    }

    fn roles(t: &Theme) -> [(&'static str, Option<Color>); 6] {
        [
            ("dim", t.dim),
            ("warn", t.warn),
            ("error", t.error),
            ("ok", t.ok),
            ("heading", t.heading),
            ("accent", t.accent),
        ]
    }

    fn fixed(role: &str, c: Option<Color>) -> u8 {
        match c {
            Some(Color::Indexed(n)) if n >= 16 => n,
            _ => panic!("{role}: expected a fixed 256-colour index, got {c:?}"),
        }
    }

    #[test]
    fn auto_with_an_unknown_background_is_readable_on_white_and_black() {
        let t = Theme::for_background("auto", Background::Unknown, true);
        for (role, c) in roles(&t) {
            let n = fixed(role, c);
            let (w, k) = (contrast_on_white(n), contrast_on_black(n));
            assert!(
                w >= 3.9 && k >= 3.9,
                "{role} (colour {n}) is {w:.1}:1 on white, {k:.1}:1 on black"
            );
        }
        assert_eq!(t.text, None);
        assert_eq!(
            t.sel_bg, None,
            "reverse video follows the terminal's own colours"
        );
        assert!(t.sel_style().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn auto_follows_a_known_background() {
        let light = Theme::for_background("auto", Background::Light, true);
        assert_eq!(
            (light.name, light.dim),
            ("auto", Theme::named("light", false, true).dim)
        );
        let dark = Theme::for_background("auto", Background::Dark, true);
        assert_eq!(dark.sel_bg, Some(Color::Indexed(236)));
        // only auto changes for an unknown background; catppuccin still assumes dark
        assert_eq!(
            Theme::for_background("catppuccin", Background::Unknown, true),
            Theme::named("catppuccin", true, true)
        );
    }

    #[test]
    fn colorfgbg_says_dark_light_or_nothing() {
        assert_eq!(parse_colorfgbg(None), Background::Unknown);
        assert_eq!(parse_colorfgbg(Some("")), Background::Unknown);
        assert_eq!(parse_colorfgbg(Some("15;0")), Background::Dark);
        assert_eq!(parse_colorfgbg(Some("0;default;8")), Background::Dark);
        assert_eq!(parse_colorfgbg(Some("0;15")), Background::Light);
        assert_eq!(parse_colorfgbg(Some("0;7")), Background::Light);
        assert_eq!(parse_colorfgbg(Some("15;default")), Background::Unknown);
    }

    #[test]
    fn every_light_theme_colour_is_fixed_and_readable_on_white() {
        let t = Theme::named("light", false, true);
        for (role, c) in roles(&t) {
            let n = fixed(role, c);
            let ratio = contrast_on_white(n);
            assert!(
                ratio >= 4.5,
                "{role} (colour {n}) is only {ratio:.1}:1 on white"
            );
        }
        assert_eq!(
            t.text, None,
            "body text keeps the terminal's own foreground"
        );
        assert_eq!(xterm_rgb(242), (108, 108, 108));
    }

    use super::*;

    #[test]
    fn precedence() {
        assert_eq!(resolve(None, "").unwrap(), "auto");
        assert_eq!(resolve(None, "Dark").unwrap(), "dark");
        assert_eq!(resolve(Some("light"), "dark").unwrap(), "light");
        assert_eq!(resolve(Some(""), "dracula").unwrap(), "dracula");
        assert!(resolve(None, "nope").is_err());
        assert_eq!(
            by_name("x").unwrap_err().to_string(),
            "unknown theme \"x\" (choose from: auto, dark, light, ansi, dracula, catppuccin)"
        );
    }

    #[test]
    fn adaptive_and_no_color() {
        assert_eq!(
            Theme::named("auto", true, true).sel_bg,
            Some(Color::Indexed(236))
        );
        assert_eq!(
            Theme::named("auto", false, true).sel_bg,
            Some(Color::Indexed(254))
        );
        assert_eq!(Theme::named("ansi", true, true).sel_bg, None);
        assert!(
            Theme::named("ansi", true, true)
                .sel_style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        let plain = Theme::named("dracula", true, false);
        assert!(
            !plain.color && plain.fresh.len() == 4 && plain.dim.is_none() && plain.text.is_none()
        );
        assert_eq!(Theme::named("dark", true, true).fresh.len(), 9);
        assert_eq!(
            Theme::named("catppuccin", false, true).accent,
            Some(Color::Rgb(0x88, 0x39, 0xef))
        );
    }

    #[test]
    fn plain_preview_has_no_escapes() {
        let mut buf = Vec::new();
        print_themes(&mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains('\x1b'));
        assert!(s.starts_with("auto\n  4m Sample post @you.bsky.social\n▸ 4m Sample post"));
        assert_eq!(s.matches("fresh ").count(), 6);
    }
}
