//! Terminal-safe text helpers (port of Go `internal/termtext`, the parts Phase 0-1 need).

use std::sync::OnceLock;

use chrono::Duration;
use regex::Regex;
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

fn is_mark(w: &str) -> bool {
    w == "↗"
        || w == VIDEO_MARK
        || w == IMAGE_MARK
        || w.strip_prefix("▣×")
            .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
}

/// Joins a lone marker (`↗`, `▶`, `▣`, `▣×3`) to the word after it so a line break never
/// strands the marker at the end of a line.
fn glue_prefixes(words: Vec<&str>) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        if is_mark(words[i]) && i + 1 < words.len() {
            out.push(format!("{} {}", words[i], words[i + 1]));
            i += 2;
        } else {
            out.push(words[i].to_string());
            i += 1;
        }
    }
    out
}

/// Width of a character on its own as Go's grapheme-based measure sees it (a lone regional
/// indicator is drawn as an emoji, 2 columns).
fn rune_width(c: char) -> usize {
    if ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) {
        2
    } else {
        c.width().unwrap_or(0)
    }
}

const URL_TRAILING: &str = ".,;:!?)]}\"'";

/// URLs and bare domains with a common TLD (Go `linkRe`). Stops at whitespace and ESC.
fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)(?:https?://[^\t\n\x0c\r \x1b]+|(?-u:\b)[a-z0-9][a-z0-9-]*(?:\.[a-z0-9-]+)*\.",
            r"(?:com|org|net|io|app|dev|ai|co|me|xyz|social|blog|info|tv|ly|edu|gov|uk|de|fr|ca|au)",
            r"(?-u:\b)(?:/[^\t\n\x0c\r \x1b]*)?)"
        ))
        .expect("link regex")
    })
}

fn http_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"https?://[^\t\n\x0c\r \x1b]+").expect("http regex"))
}

fn mention_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[^A-Za-z0-9_@])@([a-z0-9][a-z0-9-]*(?:\.[a-z0-9-]+)+)")
            .expect("mention regex")
    })
}

fn url_target(url: &str) -> String {
    if url.to_lowercase().starts_with("http") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// The http(s) URLs in plain text, without trailing punctuation.
pub fn find_urls(s: &str) -> Vec<String> {
    http_re()
        .find_iter(s)
        .map(|m| {
            m.as_str()
                .trim_end_matches(|c| URL_TRAILING.contains(c))
                .to_string()
        })
        .collect()
}

/// Handles of `@handle.domain` mentions in text (without the `@`).
pub fn find_mentions(s: &str) -> Vec<String> {
    mention_re()
        .captures_iter(s)
        .map(|c| c[1].to_string())
        .collect()
}

/// A hyperlink over `len` display columns starting at `col` of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub col: usize,
    pub len: usize,
    pub url: String,
}

/// One wrapped line and the links in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WrapLine {
    pub text: String,
    pub links: Vec<LinkSpan>,
}

impl WrapLine {
    /// The line with OSC 8 escapes inserted (Go's `Wrap` output format).
    pub fn to_osc(&self) -> String {
        let mut out = String::new();
        let mut col = 0;
        let mut links = self.links.iter().peekable();
        let mut chunk = String::new();
        let flush_to = |out: &mut String, chunk: &mut String| {
            out.push_str(chunk);
            chunk.clear();
        };
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if let Some(l) = links.peek().filter(|l| l.col == col) {
                flush_to(&mut out, &mut chunk);
                let (mut w, mut text) = (0, String::new());
                while i < chars.len() && w < l.len {
                    w += chars[i].width().unwrap_or(0);
                    text.push(chars[i]);
                    i += 1;
                }
                // zero-width characters directly after the span belong to it
                while i < chars.len() && chars[i].width().unwrap_or(0) == 0 && w >= l.len {
                    text.push(chars[i]);
                    i += 1;
                }
                col += w;
                out.push_str(&format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", l.url, text));
                links.next();
                continue;
            }
            chunk.push(chars[i]);
            col += chars[i].width().unwrap_or(0);
            i += 1;
        }
        flush_to(&mut out, &mut chunk);
        out
    }
}

struct Piece {
    text: String,
    target: Option<String>,
}

/// Splits a word into plain and URL pieces (URLs carry their target).
fn split_word(word: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut pos = 0;
    for m in link_re().find_iter(word) {
        let (mut start, end) = (m.start(), m.end());
        let url = word[start..end].trim_end_matches(|c| URL_TRAILING.contains(c));
        let mut target = url_target(url);
        let mut text = url.to_string();
        if start > 0 && word.as_bytes()[start - 1] == b'@' && !url.contains('/') {
            // an @mention: link to the profile and keep the @ in the link text
            start -= 1;
            text = format!("@{url}");
            target = format!("https://bsky.app/profile/{url}");
        }
        if start > pos {
            out.push(Piece {
                text: word[pos..start].to_string(),
                target: None,
            });
        }
        out.push(Piece {
            text,
            target: Some(target),
        });
        let tail = &word[m.start() + url.len()..end];
        if !tail.is_empty() {
            out.push(Piece {
                text: tail.to_string(),
                target: None,
            });
        }
        pos = end;
    }
    if pos < word.len() {
        out.push(Piece {
            text: word[pos..].to_string(),
            target: None,
        });
    }
    out
}

/// Word-wraps `text` to `width` columns (minimum 8). URLs are never shortened: one too long
/// for a line is split across lines and every fragment links to the full URL. Newlines in
/// `text` are kept. Remote controls are stripped first.
pub fn wrap_linked(text: &str, width: i64) -> Vec<WrapLine> {
    let width = width.max(8) as usize;
    let text = sanitize(text);
    let mut lines: Vec<WrapLine> = Vec::new();
    for para in text.split('\n') {
        let words = glue_prefixes(para.split_whitespace().collect());
        if words.is_empty() {
            lines.push(WrapLine::default());
            continue;
        }
        let mut cur = WrapLine::default();
        let mut cur_w = 0usize;
        for word in &words {
            let pieces = split_word(word);
            let word_w: usize = pieces.iter().map(|p| p.text.width()).sum();
            if cur_w > 0 && cur_w + 1 + word_w > width {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if cur_w > 0 {
                cur.text.push(' ');
                cur_w += 1;
            }
            for p in &pieces {
                let rs: Vec<char> = p.text.chars().collect();
                let mut i = 0;
                while i < rs.len() {
                    if cur_w >= width {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    // take as many characters as still fit (zero-width ones stay attached)
                    let (mut j, mut w) = (i, 0);
                    while j < rs.len() {
                        let rw = rune_width(rs[j]);
                        if cur_w + w + rw > width {
                            break;
                        }
                        w += rw;
                        j += 1;
                    }
                    if j == i {
                        // the next character does not fit in what is left of this line
                        if cur_w > 0 {
                            lines.push(std::mem::take(&mut cur));
                            cur_w = 0;
                            continue;
                        }
                        j = i + 1;
                        w = rune_width(rs[i]);
                    }
                    if let Some(target) = &p.target {
                        cur.links.push(LinkSpan {
                            col: cur_w,
                            len: w,
                            url: target.clone(),
                        });
                    }
                    cur.text.extend(&rs[i..j]);
                    cur_w += w;
                    i = j;
                }
            }
        }
        lines.push(cur);
    }
    lines
}

/// Plain-text wrapping (see [`wrap_linked`]).
pub fn wrap(text: &str, width: i64) -> Vec<String> {
    wrap_linked(text, width)
        .into_iter()
        .map(|l| l.text)
        .collect()
}

/// Hard-wraps `text` to `w` columns keeping at most `n` lines; the last ends in `…` when
/// something was cut.
pub fn clip(text: &str, w: i64, n: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = wrap(text, w);
    if lines.len() > n {
        lines.truncate(n);
        let last = format!("{} …", lines[n - 1]);
        lines[n - 1] = truncate_width(&last, w.max(0) as usize);
    }
    lines
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

    /// Expected values were produced by Go's `termtext.Wrap` + `tui.clip` (tests/compat/wrap_cases.json).
    #[test]
    fn wrap_and_clip_match_go() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/compat/wrap_cases.json")).unwrap();
        assert!(cases.len() > 300);
        for c in cases {
            let (text, w, n) = (
                c["text"].as_str().unwrap(),
                c["w"].as_i64().unwrap(),
                c["n"].as_u64().unwrap() as usize,
            );
            let want = |k: &str| -> Vec<String> {
                c[k].as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect()
            };
            assert_eq!(wrap(text, w), want("wrap"), "wrap({text:?}, {w})");
            let osc: Vec<String> = wrap_linked(text, w).iter().map(WrapLine::to_osc).collect();
            assert_eq!(osc, want("osc"), "hyperlinks in wrap({text:?}, {w})");
            assert_eq!(find_urls(text), want("urls"), "find_urls({text:?})");
            assert_eq!(
                find_mentions(text),
                want("mentions"),
                "find_mentions({text:?})"
            );
            assert_eq!(clip(text, w, n), want("clip"), "clip({text:?}, {w}, {n})");
        }
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
