//! OSC 8 hyperlinks on top of ratatui's cell buffer.
//!
//! Strategy (decided in the Phase 2 spike): ratatui's diffing measures each cell's symbol
//! with `unicode-width`, so raw escape sequences inside symbols corrupt its width
//! bookkeeping. Instead the link targets stay out of the buffer. After every draw a small
//! pass re-prints just the link cells, each wrapped in its own OSC 8 open/close, straight to
//! the terminal:
//!
//! * link cells are re-printed on **every** frame, so a link that kept its text but changed
//!   its target (a different post scrolled into the same row) is never stale;
//! * cells that were links last frame but are not now are re-printed **plain**, which
//!   removes the hyperlink the terminal still attaches to them;
//! * each cell carries its own open/close, so ratatui's cursor moves and SGR changes
//!   between cells cannot leak a link onto neighbouring text;
//! * nothing is emitted when links are off (`TERM=dumb`/`linux`), leaving plain text.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

use super::render::LinkRegion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    Osc8,
    Off,
}

impl LinkMode {
    /// Links are on unless the terminal is known not to understand them.
    pub fn detect() -> Self {
        match std::env::var("TERM").as_deref() {
            Ok("dumb") | Ok("linux") => LinkMode::Off,
            _ => LinkMode::Osc8,
        }
    }
}

pub struct LinkPass {
    mode: LinkMode,
    prev: Vec<LinkRegion>,
}

impl LinkPass {
    pub fn new(mode: LinkMode) -> Self {
        LinkPass {
            mode,
            prev: Vec::new(),
        }
    }

    /// Bytes to write after the frame in `buf` was flushed. `current` are this frame's links.
    pub fn bytes(&mut self, buf: &Buffer, current: Vec<LinkRegion>) -> Vec<u8> {
        if self.mode == LinkMode::Off {
            return Vec::new();
        }
        let current: Vec<LinkRegion> = current.into_iter().filter(|r| safe_url(&r.url)).collect();
        let mut out = Vec::new();
        let mut linked: HashSet<(u16, u16)> = HashSet::new();
        for r in &current {
            for dx in 0..r.len {
                linked.insert((r.x + dx, r.y));
            }
        }
        // stale links first, so a region that moved is cleared before it is re-drawn
        for r in &self.prev {
            for dx in 0..r.len {
                let pos = (r.x + dx, r.y);
                if !linked.contains(&pos) {
                    emit(&mut out, buf, pos, None);
                }
            }
        }
        for r in &current {
            for dx in 0..r.len {
                emit(&mut out, buf, (r.x + dx, r.y), Some(&r.url));
            }
        }
        self.prev = current;
        out
    }
}

/// Targets with control characters could break out of the escape sequence.
fn safe_url(url: &str) -> bool {
    !url.is_empty() && !url.chars().any(|c| c.is_control())
}

fn emit(out: &mut Vec<u8>, buf: &Buffer, (x, y): (u16, u16), url: Option<&str>) {
    let a = buf.area;
    if x < a.x || y < a.y || x >= a.x + a.width || y >= a.y + a.height {
        return;
    }
    let cell = &buf[(x, y)];
    out.extend(format!("\x1b[{};{}H", y - a.y + 1, x - a.x + 1).bytes());
    let style = cell.style();
    let sgr = sgr(style);
    out.extend(sgr.bytes());
    if let Some(url) = url {
        out.extend(format!("\x1b]8;;{url}\x1b\\").bytes());
    }
    out.extend(cell.symbol().bytes());
    if url.is_some() {
        out.extend(b"\x1b]8;;\x1b\\");
    }
    if !sgr.is_empty() {
        out.extend(b"\x1b[0m");
    }
}

fn color_code(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 40 } else { 30 };
    Some(match c {
        Color::Reset => return None,
        Color::Black => format!("{base}"),
        Color::Red => format!("{}", base + 1),
        Color::Green => format!("{}", base + 2),
        Color::Yellow => format!("{}", base + 3),
        Color::Blue => format!("{}", base + 4),
        Color::Magenta => format!("{}", base + 5),
        Color::Cyan => format!("{}", base + 6),
        Color::Gray => format!("{}", base + 7),
        Color::DarkGray => format!("{}", base + 60),
        Color::LightRed => format!("{}", base + 61),
        Color::LightGreen => format!("{}", base + 62),
        Color::LightYellow => format!("{}", base + 63),
        Color::LightBlue => format!("{}", base + 64),
        Color::LightMagenta => format!("{}", base + 65),
        Color::LightCyan => format!("{}", base + 66),
        Color::White => format!("{}", base + 67),
        Color::Indexed(n) => format!("{};5;{n}", base + 8),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
    })
}

/// SGR escape for a style (empty when the style is plain).
pub fn sgr(style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    for (m, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::REVERSED, "7"),
    ] {
        if style.add_modifier.contains(m) && !style.sub_modifier.contains(m) {
            codes.push(code.into());
        }
    }
    codes.extend(style.fg.and_then(|c| color_code(c, false)));
    codes.extend(style.bg.and_then(|c| color_code(c, true)));
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn buf() -> Buffer {
        let mut b = Buffer::empty(Rect::new(0, 0, 20, 3));
        b.set_string(2, 1, "12m", Style::new().fg(Color::Indexed(8)));
        b.set_string(8, 2, "日本", Style::new());
        b
    }

    fn region(x: u16, y: u16, len: u16, url: &str) -> LinkRegion {
        LinkRegion {
            x,
            y,
            len,
            url: url.into(),
        }
    }

    fn s(b: Vec<u8>) -> String {
        String::from_utf8(b).unwrap()
    }

    #[test]
    fn wraps_each_cell_in_its_own_link() {
        let mut p = LinkPass::new(LinkMode::Osc8);
        let out = s(p.bytes(&buf(), vec![region(2, 1, 3, "https://bsky.app/x")]));
        let cell = |col: u16, ch: &str| {
            format!(
                "\x1b[2;{col}H\x1b[38;5;8m\x1b]8;;https://bsky.app/x\x1b\\{ch}\x1b]8;;\x1b\\\x1b[0m"
            )
        };
        assert_eq!(out, cell(3, "1") + &cell(4, "2") + &cell(5, "m"));
    }

    #[test]
    fn relinks_every_frame_so_changed_targets_are_never_stale() {
        let mut p = LinkPass::new(LinkMode::Osc8);
        let a = s(p.bytes(&buf(), vec![region(2, 1, 3, "https://a")]));
        let b = s(p.bytes(&buf(), vec![region(2, 1, 3, "https://b")]));
        assert!(a.contains("https://a") && !a.contains("https://b"));
        assert!(b.contains("https://b") && !b.contains("https://a"));
        let again = s(p.bytes(&buf(), vec![region(2, 1, 3, "https://b")]));
        assert_eq!(again, b, "unchanged links are still re-emitted");
    }

    #[test]
    fn cells_that_stopped_being_links_are_reprinted_plain() {
        let mut p = LinkPass::new(LinkMode::Osc8);
        p.bytes(&buf(), vec![region(2, 1, 3, "https://a")]);
        let out = s(p.bytes(&buf(), vec![region(3, 1, 2, "https://a")]));
        // column 3 (x=2) lost its link: plain, no OSC 8
        assert!(out.starts_with("\x1b[2;3H\x1b[38;5;8m1\x1b[0m"), "{out:?}");
        assert_eq!(out.matches("\x1b]8;;https").count(), 2);
        let gone = s(p.bytes(&buf(), vec![]));
        assert!(
            !gone.contains("\x1b]8;;"),
            "no links left: only plain re-prints"
        );
        assert!(gone.contains('m') && !gone.is_empty());
        assert!(
            p.bytes(&buf(), vec![]).is_empty(),
            "and nothing once cleaned up"
        );
    }

    #[test]
    fn off_mode_and_unsafe_targets_emit_nothing() {
        let mut off = LinkPass::new(LinkMode::Off);
        assert!(
            off.bytes(&buf(), vec![region(2, 1, 3, "https://a")])
                .is_empty()
        );
        let mut p = LinkPass::new(LinkMode::Osc8);
        assert!(
            p.bytes(&buf(), vec![region(2, 1, 3, "https://a\x1b\\evil")])
                .is_empty()
        );
        assert!(p.bytes(&buf(), vec![region(2, 1, 3, "")]).is_empty());
    }

    #[test]
    fn regions_outside_the_buffer_are_ignored() {
        let mut p = LinkPass::new(LinkMode::Osc8);
        let out = s(p.bytes(&buf(), vec![region(18, 1, 5, "https://a")]));
        assert_eq!(
            out.matches("\x1b]8;;https").count(),
            2,
            "only x=18 and x=19 exist"
        );
    }

    #[test]
    fn mode_detection() {
        // not asserting on the ambient TERM: just that detect() never panics
        let _ = LinkMode::detect();
    }
}
