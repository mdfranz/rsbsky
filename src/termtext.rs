//! Terminal-safe text helpers (port of Go `internal/termtext`, the parts Phase 0-1 need).

use chrono::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Single-width media markers.
pub const IMAGE_MARK: &str = "▣";
pub const VIDEO_MARK: &str = "▶";

#[derive(Clone, Copy, PartialEq)]
enum Esc {
    Ground,
    Escape,
    EscapeInter,
    Csi,
    Osc,
    /// DCS / SOS / PM / APC string: ends at ST, not at BEL.
    Str,
}

/// Strips ANSI escape sequences, approximating `charmbracelet/x/ansi.Strip`.
/// C0 controls met inside a sequence are kept (the caller removes them), as in Go.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut st = Esc::Ground;
    for c in s.chars() {
        let u = c as u32;
        // CAN and SUB cancel any sequence in progress.
        if st != Esc::Ground && (u == 0x18 || u == 0x1a) {
            st = Esc::Ground;
            continue;
        }
        if u == 0x1b {
            st = Esc::Escape;
            continue;
        }
        match st {
            Esc::Ground => out.push(c),
            Esc::Escape | Esc::EscapeInter => match u {
                0..=0x1f => out.push(c),
                0x20..=0x2f => st = Esc::EscapeInter,
                0x5b if st == Esc::Escape => st = Esc::Csi,
                0x5d if st == Esc::Escape => st = Esc::Osc,
                0x50 | 0x58 | 0x5e | 0x5f if st == Esc::Escape => st = Esc::Str,
                0x30..=0x7e => st = Esc::Ground,
                _ => {} // DEL and anything else is ignored
            },
            Esc::Csi => match u {
                0..=0x1f => out.push(c),
                0x40..=0x7e => st = Esc::Ground,
                _ => {} // parameters, intermediates, DEL
            },
            Esc::Osc => {
                if u == 0x07 || u == 0x9c {
                    st = Esc::Ground;
                }
            }
            Esc::Str => {
                if u == 0x9c {
                    st = Esc::Ground;
                }
            }
        }
    }
    out
}

/// Removes ANSI sequences and control characters (except `\n` and `\t`) from remote text.
pub fn sanitize(s: &str) -> String {
    strip_ansi(s)
        .chars()
        .filter(|&c| !c.is_control() || c == '\n' || c == '\t')
        .collect()
}

/// Display width of `s` in terminal columns.
pub fn width(s: &str) -> usize {
    s.width()
}

/// Cuts `s` to at most `w` columns, ending in "…" when something was cut.
pub fn truncate_width(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if width(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// A duration as one short unit: 5m, 3h, 12d or 7mo ("?" when unknown).
pub fn human_age(d: Duration) -> String {
    if d <= Duration::zero() {
        return "?".into();
    }
    let secs = d.num_seconds();
    if d < Duration::hours(1) {
        format!("{}m", secs / 60)
    } else if d < Duration::hours(48) {
        format!("{}h", secs / 3600)
    } else if d < Duration::days(90) {
        format!("{}d", secs / 86400)
    } else {
        format!("{}mo", secs / 86400 / 30)
    }
}

/// Collapses runs of whitespace into single spaces (Go `strings.Join(strings.Fields(s), " ")`).
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_sequences_and_controls() {
        assert_eq!(sanitize("a\x1b[31mred\x1b[0m b"), "ared b");
        assert_eq!(sanitize("x\x1b]0;title\x07y"), "xy");
        assert_eq!(
            sanitize("x\x1b]8;;http://e\x1b\\link\x1b]8;;\x1b\\"),
            "xlink"
        );
        assert_eq!(sanitize("tab\tnl\nbell\x07nul\0"), "tab\tnl\nbellnul");
        assert_eq!(sanitize("a\u{9b}b"), "ab");
        assert_eq!(sanitize("日本語 ▣"), "日本語 ▣");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate_width("hello", 5), "hello");
        assert_eq!(truncate_width("hello world", 6), "hello…");
        assert_eq!(truncate_width("日本語です", 6), "日本…");
        assert_eq!(truncate_width("x", 0), "");
    }

    #[test]
    fn ages() {
        assert_eq!(human_age(Duration::minutes(5)), "5m");
        assert_eq!(human_age(Duration::hours(47)), "47h");
        assert_eq!(human_age(Duration::hours(48)), "2d");
        assert_eq!(human_age(Duration::days(89)), "89d");
        assert_eq!(human_age(Duration::days(400)), "13mo");
        assert_eq!(human_age(Duration::seconds(-3)), "?");
    }
}
