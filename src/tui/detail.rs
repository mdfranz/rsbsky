//! Lines for the post detail view: the author's profile panel on the left and the post,
//! its engagement and the author's recent posts on the right.
//!
//! Port of `profilePanel`, `currentPost`, `statsLine`, `recentLines`, `renderPost` and
//! `postRight` from Go `internal/tui/ui.go`. Lines are structured ([`PLine`]) rather than
//! pre-rendered strings: styled spans plus hyperlink spans, so links can be laid over the
//! frame by `links.rs` after drawing.

use chrono::DateTime;
use ratatui::style::Style;
use ratatui::text::Span;

use super::feed::{FeedApp, Now, ProfileSlot};
use super::render::{age_of, post_link};
use crate::bsky::{FeedItem, Post, Profile};
use crate::store;
use crate::termtext::{LinkSpan, WrapLine, human_age, wrap_linked};

/// A panel line: styled spans and the hyperlinks within them (columns relative to the line).
#[derive(Debug, Clone, Default)]
pub struct PLine {
    pub spans: Vec<Span<'static>>,
    pub links: Vec<LinkSpan>,
}

impl PLine {
    pub fn blank() -> Self {
        PLine::default()
    }

    pub fn text(s: impl Into<String>, style: Style) -> Self {
        let mut l = PLine::default();
        l.push(s, style);
        l
    }

    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|s| crate::termtext::width(&s.content))
            .sum()
    }

    /// Appends text and returns the column it starts at.
    pub fn push(&mut self, s: impl Into<String>, style: Style) -> usize {
        let col = self.width();
        self.spans.push(Span::styled(s.into(), style));
        col
    }

    /// Appends text that is a hyperlink to `url`.
    pub fn push_link(&mut self, s: impl Into<String>, style: Style, url: String) {
        let s = s.into();
        let len = crate::termtext::width(&s);
        let col = self.push(s, style);
        self.links.push(LinkSpan { col, len, url });
    }

    /// Appends a wrapped line (its own links are shifted into place).
    pub fn push_wrapped(&mut self, w: &WrapLine, style: Style) {
        let col = self.push(w.text.clone(), style);
        self.links.extend(w.links.iter().map(|l| LinkSpan {
            col: col + l.col,
            len: l.len,
            url: l.url.clone(),
        }));
    }

    /// Puts `s` in front of the line, moving its links right.
    pub fn prepend(&mut self, s: &str, style: Style) {
        let w = crate::termtext::width(s);
        self.spans.insert(0, Span::styled(s.to_string(), style));
        for l in &mut self.links {
            l.col += w;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.content.is_empty())
    }

    /// Plain text (for tests).
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}

fn count(n: i64, one: &str) -> String {
    match (n, one) {
        (1, _) => format!("1 {one}"),
        (_, "reply") => format!("{n} replies"),
        _ => format!("{n} {one}s"),
    }
}

impl FeedApp {
    /// Wraps `text` into `out`, one line per wrapped line, behind `prefix`.
    fn body(&self, out: &mut Vec<PLine>, text: &str, w: i64, style: Option<Style>, prefix: &str) {
        let indent = "  ";
        for l in wrap_linked(text, w - indent.len() as i64 - prefix.len() as i64) {
            let mut line = PLine::text(format!("{indent}{prefix}"), Style::new());
            line.push_wrapped(&l, style.unwrap_or_default());
            out.push(line);
        }
    }

    /// A post in full: a dim header, then the wrapped text indented. `full` adds reply
    /// context and image links (used for the post under the cursor).
    pub fn render_post(&self, p: &Post, w: i64, full: bool, now: Now) -> Vec<PLine> {
        let dim = self.theme.dim_style();
        let kind = if p.repost {
            " · repost"
        } else if p.reply {
            " · reply"
        } else {
            ""
        };
        let age = age_of(&p.created_at, now);
        let mut lines = vec![PLine::text(format!("{age} ago{kind}"), dim)];
        if full && !p.parent_handle.is_empty() {
            let text = format!(
                "↳ replying to @{}: {}",
                p.parent_handle,
                p.parent_text.trim()
            );
            self.body(&mut lines, &text, w, Some(dim), "");
        }
        if !p.text.trim().is_empty() {
            self.body(&mut lines, p.text.trim(), w, None, "");
        }
        for a in &p.attach {
            self.body(&mut lines, a, w, Some(dim), "");
        }
        if let Some(c) = &p.link {
            let label = c.label(&p.text);
            if !label.is_empty() {
                self.body(&mut lines, &label, w, Some(dim), "");
            }
        }
        if !p.quote.is_empty() {
            self.body(&mut lines, &p.quote, w, Some(dim), "▎ ");
        }
        if full {
            for (i, u) in p.images.iter().enumerate() {
                self.body(
                    &mut lines,
                    &format!("image {}: {u}", i + 1),
                    w,
                    Some(dim),
                    "",
                );
            }
        }
        if lines.len() == 1 {
            lines.push(PLine::text("  (no text)", Style::new()));
        }
        lines.push(PLine::blank());
        lines
    }

    /// Engagement counts and a link to the post on bsky.app. When that does not fit in
    /// `avail` columns the link moves to a line of its own instead of running off the panel.
    pub fn stats_lines(&self, it: &FeedItem, avail: usize) -> Vec<PLine> {
        let th = &self.theme;
        let s = it.stats;
        let mut line = PLine::default();
        if it.like.is_empty() {
            line.push(count(s.likes, "like"), th.dim_style());
        } else {
            line.push(
                format!("♥ {} (you liked this)", count(s.likes, "like")),
                th.err_style(),
            );
        }
        let rest = format!(
            " · {} · {} · {}",
            count(s.reposts, "repost"),
            count(s.replies, "reply"),
            count(s.quotes, "quote")
        );
        line.push(rest, th.dim_style());
        let link = "open post ↗";
        if line.width() + 3 + crate::termtext::width(link) <= avail {
            line.push(" · ", th.dim_style());
            line.push_link(link, th.accent_style(), post_link(it));
            return vec![line];
        }
        let mut second = PLine::default();
        second.push_link(link, th.accent_style(), post_link(it));
        vec![line, second]
    }

    /// The post being looked at: a heading and an accent bar down its side.
    pub fn current_post(
        &self,
        title: &str,
        p: &Post,
        it: &FeedItem,
        cw: i64,
        now: Now,
    ) -> Vec<PLine> {
        let th = &self.theme;
        let mut lines = vec![PLine::text(format!("▸ {title}"), th.heading_style())];
        for mut l in self.render_post(p, (cw - 2).max(10), true, now) {
            if l.is_empty() {
                continue;
            }
            l.prepend("┃ ", th.accent_style());
            lines.push(l);
        }
        if !it.uri.is_empty() {
            for mut l in self.stats_lines(it, (cw - 2).max(10) as usize) {
                l.prepend("┃ ", th.accent_style());
                lines.push(l);
            }
        }
        lines.push(PLine::blank());
        lines.push(PLine::text(
            "─".repeat((cw - 1).max(1) as usize),
            th.dim_style(),
        ));
        lines.push(PLine::blank());
        lines
    }

    /// The author's latest posts, leaving out `cur` (shown above as the current post).
    pub fn recent_lines(&self, did: &str, cw: i64, cur: &Post, now: Now) -> Vec<PLine> {
        let mut lines = vec![
            PLine::text("Recent posts", self.theme.heading_style()),
            PLine::blank(),
        ];
        match self.posts.get(did) {
            None | Some(super::feed::PostsSlot::Loading) => {
                lines.push(PLine::text(
                    format!("{} loading posts…", self.spinner()),
                    self.theme.base(),
                ));
            }
            Some(super::feed::PostsSlot::Loaded(pp)) if pp.is_empty() => {
                lines.push(PLine::text("(no posts)", self.theme.dim_style()));
            }
            Some(super::feed::PostsSlot::Loaded(pp)) => {
                for post in pp {
                    if post.created_at == cur.created_at && post.text == cur.text {
                        continue;
                    }
                    lines.extend(self.render_post(post, cw, false, now));
                }
            }
        }
        lines
    }

    /// The scrollable right-hand panel for the post under the cursor.
    pub fn post_right(&self, cw: i64, now: Now) -> Vec<PLine> {
        let it = &self.items[self.cur];
        let mut lines = Vec::new();
        if !it.by.is_empty() {
            let mut by = format!("↻ Reposted by @{}", it.by);
            if !it.reposted_at.is_empty() {
                by.push_str(&format!(" · {} ago", age_of(&it.reposted_at, now)));
            }
            lines.push(PLine::text(by, self.theme.accent_style()));
            lines.push(PLine::blank());
        }
        let title = format!("This post · {} of {}", self.cur + 1, self.items.len());
        lines.extend(self.current_post(&title, &it.post, it, cw, now));
        lines.extend(self.recent_lines(&it.author.did, cw, &it.post, now));
        lines
    }

    fn decision_badge(&self, did: &str) -> Option<PLine> {
        let th = &self.theme;
        let (text, style) = match self.decisions.get(did).map(String::as_str)? {
            store::BLOCKED => ("● blocked", th.err_style()),
            store::FOLLOWED => ("● followed", th.ok_style()),
            store::KEPT => ("● kept", th.ok_style()),
            store::SKIPPED => ("● skipped", th.dim_style()),
            _ => return None,
        };
        Some(PLine::text(text, style))
    }

    /// The account being looked at: the author of the current post, with the fetched
    /// profile replacing the timeline summary once it has loaded.
    pub fn subject(&self) -> Option<Profile> {
        let p = self.items.get(self.cur)?.author.clone();
        Some(match self.profiles.get(&p.did) {
            Some(ProfileSlot::Loaded(full)) => (**full).clone(),
            _ => p,
        })
    }

    /// The left-hand profile panel; `pos` is "N of M".
    pub fn profile_panel(&self, cw: i64, pos: &str, now: Now) -> Vec<PLine> {
        let th = &self.theme;
        let Some(p) = self.subject() else {
            return Vec::new();
        };
        let name = if p.display_name.is_empty() {
            "(no display name)"
        } else {
            p.display_name.as_str()
        };
        let mut lines: Vec<PLine> = wrap_linked(name, cw - 12)
            .iter()
            .map(|w| {
                let mut l = PLine::default();
                l.push_wrapped(w, th.heading_style());
                l
            })
            .collect();
        if let Some(last) = lines.last_mut() {
            last.push("  ", Style::new());
            last.push(pos, th.dim_style());
        }
        lines.push(PLine::text(format!("@{}", p.handle), th.accent_style()));
        let mut open = PLine::default();
        open.push_link(
            "open profile ↗",
            th.dim_style(),
            format!("https://bsky.app/profile/{}", p.handle),
        );
        lines.push(open);
        if let Some(b) = self.decision_badge(&p.did) {
            lines.push(b);
        } else if !p.viewer.following.is_empty() {
            lines.push(PLine::text("● you follow them", th.dim_style()));
        }
        lines.push(PLine::blank());
        if !matches!(self.profiles.get(&p.did), Some(ProfileSlot::Loaded(_))) {
            lines.push(PLine::text(
                format!("{} loading profile…", self.spinner()),
                th.base(),
            ));
            return lines;
        }
        let mut row = |label: &str, val: Vec<(String, Style)>| {
            let mut l = PLine::text(format!("{label:<12}"), th.dim_style());
            l.push(" ", Style::new());
            for (t, s) in val {
                l.push(t, s);
            }
            lines.push(l);
        };
        let plain = |s: String| vec![(s, Style::new())];
        let age = p.age_at(now.wall);
        row("Account age", plain(human_age(age)));
        if let Ok(t) = DateTime::parse_from_rfc3339(&p.created_at) {
            row("Joined", plain(t.format("%Y-%m-%d").to_string()));
        }
        row(
            "Avatar",
            if p.avatar.is_empty() {
                vec![("no".into(), th.warn_style())]
            } else {
                plain("yes".into())
            },
        );
        row("Posts", plain(p.posts_count.to_string()));
        row("Followers", plain(p.followers_count.to_string()));
        row("Following", plain(p.follows_count.to_string()));
        let days = age.num_milliseconds() as f64 / 3_600_000.0 / 24.0;
        if days >= 1.0 {
            row(
                "Posts/day",
                plain(format!("{:.1}", p.posts_count as f64 / days)),
            );
        }
        if p.follows_count > 0 {
            row(
                "Fol. ratio",
                plain(format!(
                    "{:.1}",
                    p.followers_count as f64 / p.follows_count as f64
                )),
            );
        }
        if !p.viewer.followed_by.is_empty() {
            row("Relation", plain("follows you".into()));
        }
        lines.push(PLine::blank());
        let bio = p.description.trim();
        if bio.is_empty() {
            lines.push(PLine::text("(no bio)", th.dim_style()));
        } else {
            for w in wrap_linked(bio, cw) {
                let mut l = PLine::default();
                l.push_wrapped(&w, Style::new());
                lines.push(l);
            }
        }
        lines
    }
}
