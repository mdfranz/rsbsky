//! Drawing the Following view into a ratatui buffer.
//!
//! The layout follows the Go view: a title row, a blank row, the list, a blank row, then
//! the error / status / key-hint rows pinned to the bottom. Hyperlink targets are not put
//! in the buffer: [`render`] returns them as [`LinkRegion`]s and the link pass in
//! `links.rs` wraps those cells in OSC 8 after each draw.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::feed::{FeedApp, Now};
use crate::bsky::{self, FeedItem};
use crate::store;
use crate::termtext::{human_age, sanitize, truncate_width, width};

/// A run of cells that should be a hyperlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRegion {
    pub x: u16,
    pub y: u16,
    /// Width in cells.
    pub len: u16,
    pub url: String,
}

const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
/// Text lines sit under the author's name: lead (2) + age (5) + space.
const INDENT: &str = "      ";

/// How long ago `s` (RFC 3339) was, or `?`.
pub fn age_of(s: &str, now: Now) -> String {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| human_age(now.wall.signed_duration_since(t)))
        .unwrap_or_else(|_| "?".into())
}

/// The post's page on bsky.app.
pub fn post_link(it: &FeedItem) -> String {
    if it.author.handle.is_empty() {
        return bsky::post_url(&it.uri);
    }
    let rkey = it.uri.rsplit('/').next().unwrap_or("");
    format!(
        "https://bsky.app/profile/{}/post/{}",
        it.author.handle, rkey
    )
}

struct Row {
    line: Line<'static>,
    /// Column, width and target of the age link.
    link: Option<(u16, u16, String)>,
    /// Fill the whole row with the selection style.
    selected: bool,
}

fn span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

fn pad(s: &str, n: usize) -> String {
    format!("{s}{}", " ".repeat(n.saturating_sub(width(s))))
}

impl FeedApp {
    /// Renders timeline item `i` at width `w`: header, text, quote, blank separator.
    fn item_rows(&self, i: usize, w: i64, now: Now) -> Vec<Row> {
        let th = &self.theme;
        let it = &self.items[i];
        let mut age = age_of(&it.post.created_at, now);
        if !it.by.is_empty() && !it.reposted_at.is_empty() {
            // when it reached your timeline; the post's own age becomes a tag
            age = age_of(&it.reposted_at, now);
        }
        let threaded = self.in_thread(i);
        let (gutter, gw) = if threaded { ("    ", 4) } else { ("", 0) };
        let name = if it.author.display_name.is_empty() {
            &it.author.handle
        } else {
            &it.author.display_name
        };
        let who = format!("{} (@{})", name, it.author.handle);

        let heading_plain = th.heading_style().remove_modifier(Modifier::BOLD);
        let mut tags: Vec<(String, Style)> = Vec::new();
        let mut written_idx = None;
        if !it.by.is_empty() {
            tags.push((format!("↻ reposted by @{}", it.by), th.accent_style()));
            let posted = age_of(&it.post.created_at, now);
            if !it.reposted_at.is_empty() && posted != age {
                written_idx = Some(tags.len());
                tags.push((
                    format!("· written {posted} ago"),
                    heading_plain.add_modifier(Modifier::ITALIC),
                ));
            }
        }
        if it.post.reply {
            if it.post.parent_handle.is_empty() {
                tags.push(("reply".into(), th.dim_style()));
            } else {
                tags.push((
                    format!("↳ replying to @{}", it.post.parent_handle),
                    th.dim_style(),
                ));
            }
        }
        if !it.like.is_empty() {
            tags.push(("♥".into(), th.err_style()));
        }
        let key = it.key();
        let fresh = self.is_fresh(&key, now);
        let fresh_style = Style::new().add_modifier(Modifier::BOLD).patch(
            self.fresh_color(&key, now)
                .map_or_else(Style::new, |c| Style::new().fg(c)),
        );
        if fresh {
            tags.push(("new".into(), fresh_style));
        }
        match self.decisions.get(&it.author.did).map(String::as_str) {
            Some(store::BLOCKED) => tags.push(("blocked".into(), th.err_style())),
            Some(store::FOLLOWED) => tags.push(("followed".into(), th.ok_style())),
            Some(store::KEPT) => tags.push(("kept".into(), th.ok_style())),
            _ => {}
        }
        let tail_plain = tags
            .iter()
            .map(|t| t.0.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let who_room =
            (w - (2 + gw as i64 + 5 + 1 + 1 + width(&tail_plain) as i64)).max(10) as usize;
        let who = truncate_width(&who, who_room);
        let (name_part, handle_part) = match who.strip_prefix(&format!("{name} (@")) {
            Some(_) => (who[..name.len()].to_string(), who[name.len()..].to_string()),
            None => (who.clone(), String::new()),
        };

        let bw = w - 2 - INDENT.len() as i64 - gw as i64;
        let mut body = self.body_lines(it, bw);
        if body.text.is_empty() && body.quote.is_empty() {
            body.text = vec!["(no text)".into()];
        }
        let link = (!it.uri.is_empty()).then(|| (2 + gw as u16, width(&age) as u16, post_link(it)));
        let age_padded = pad(&age, 5);
        let mut rows = Vec::new();

        if i == self.cur {
            let sel = th.sel_style();
            let handle_style = th.heading.map_or(sel, |c| sel.fg(c));
            let mut spans = vec![
                span(format!("▸ {gutter}{age_padded} "), sel),
                span(name_part, sel.add_modifier(Modifier::BOLD)),
                span(handle_part, handle_style),
            ];
            for (j, (text, st)) in tags.iter().enumerate() {
                let style = if Some(j) == written_idx {
                    sel.patch(*st)
                } else {
                    sel
                };
                spans.push(span(" ", sel));
                spans.push(span(text.clone(), style));
            }
            rows.push(Row {
                line: Line::from(spans),
                link,
                selected: true,
            });
            for l in &body.text {
                rows.push(Row {
                    line: Line::from(span(format!("  {gutter}{INDENT}{l}"), sel)),
                    link: None,
                    selected: true,
                });
            }
            for l in &body.quote {
                rows.push(Row {
                    line: Line::from(span(format!("  {gutter}{INDENT}▎ {l}"), sel)),
                    link: None,
                    selected: true,
                });
            }
        } else {
            let mut lead = vec![
                if fresh {
                    span("┃", fresh_style)
                } else {
                    span(" ", Style::new())
                },
                span(" ", Style::new()),
            ];
            lead.push(span(gutter, Style::new()));
            let mut header = lead.clone();
            header.push(span(age_padded, th.dim_style()));
            header.push(span(" ", Style::new()));
            header.push(span(name_part, th.title()));
            header.push(span(handle_part, heading_plain));
            for (text, st) in &tags {
                header.push(span(" ", Style::new()));
                header.push(span(text.clone(), *st));
            }
            rows.push(Row {
                line: Line::from(header),
                link,
                selected: false,
            });
            for l in &body.text {
                let mut spans = lead.clone();
                spans.push(span(format!("{INDENT}{l}"), th.dim_style()));
                rows.push(Row {
                    line: Line::from(spans),
                    link: None,
                    selected: false,
                });
            }
            for l in &body.quote {
                let mut spans = lead.clone();
                spans.push(span(INDENT, Style::new()));
                spans.push(span("▎ ", th.accent_style()));
                spans.push(span(l.clone(), th.dim_style()));
                rows.push(Row {
                    line: Line::from(spans),
                    link: None,
                    selected: false,
                });
            }
        }
        rows.push(Row {
            line: Line::default(),
            link: None,
            selected: false,
        });
        rows
    }

    fn tab_bar(&self) -> Line<'static> {
        let th = &self.theme;
        let mut label = "Following".to_string();
        let mut spans = vec![span("bskyban", th.title()), span("   ", Style::new())];
        let mut hint = None;
        if !self.new_posts.is_empty() {
            label.push_str(&format!(" (+{} new)", self.new_posts.len()));
            hint = Some(span("  R: jump to top", th.dim_style()));
        }
        spans.push(span(
            label,
            th.heading_style().add_modifier(Modifier::UNDERLINED),
        ));
        spans.extend(hint);
        if self.refresh_err.is_some() {
            spans.push(span("   ⚠ refresh failed", th.warn_style()));
        }
        Line::from(spans)
    }
}

/// Draws the whole view into `buf` (whose area is the terminal) and returns the hyperlinks.
pub fn render(app: &FeedApp, buf: &mut Buffer, now: Now) -> Vec<LinkRegion> {
    let area = buf.area;
    let th = &app.theme;
    buf.set_style(area, th.base());
    let mut links = Vec::new();

    buf.set_line(area.x, area.y, &app.tab_bar(), area.width);

    // bottom block: error, one of loading-more / status, then the hints
    let mut bottom: Vec<Line<'static>> = Vec::new();
    if let Some(e) = &app.err {
        bottom.push(Line::from(span(
            format!("error: {}", sanitize(e)),
            th.err_style(),
        )));
    }
    if app.busy {
        bottom.push(Line::from(span(
            format!("{} working…", SPINNER[app.spin_frame % SPINNER.len()]),
            th.base(),
        )));
    } else if app.more_busy {
        bottom.push(Line::from(span("loading more…", th.dim_style())));
    } else if !app.status.is_empty() {
        bottom.push(Line::from(span(app.status.clone(), th.warn_style())));
    }
    for h in app.hint_lines() {
        bottom.push(Line::from(span(h, th.dim_style())));
    }
    let room = (area.height as usize)
        .saturating_sub(bottom.len() + 1)
        .max(1); // one blank row above the footer
    let content_top = 2usize;
    let content_rows = room.saturating_sub(content_top);

    let mut y = content_top;
    let put = |buf: &mut Buffer, y: usize, line: &Line<'static>| {
        if y < content_top + content_rows {
            buf.set_line(area.x, area.y + y as u16, line, area.width);
        }
    };
    if app.follow_loading {
        let l = Line::from(span(
            format!(
                "{} loading timeline…",
                SPINNER[app.spin_frame % SPINNER.len()]
            ),
            th.base(),
        ));
        put(buf, y, &l);
    } else if app.items.is_empty() {
        put(
            buf,
            y,
            &Line::from(span("Your timeline is empty.", th.dim_style())),
        );
    } else {
        let w = app.list_width();
        let mut used = 0;
        'items: for i in app.list_top..app.items.len() {
            let rows = app.item_rows(i, w, now);
            if used > 0 && used + rows.len() > app.avail() {
                break;
            }
            used += rows.len();
            for row in rows {
                if y >= content_top + content_rows {
                    break 'items;
                }
                if row.selected {
                    buf.set_style(
                        Rect::new(area.x, area.y + y as u16, (w as u16).min(area.width), 1),
                        th.sel_style(),
                    );
                }
                put(buf, y, &row.line);
                if let Some((col, len, url)) = row.link {
                    let x = area.x + col;
                    if x + len <= area.x + area.width {
                        links.push(LinkRegion {
                            x,
                            y: area.y + y as u16,
                            len,
                            url,
                        });
                    }
                }
                y += 1;
            }
        }
    }

    let footer_top = area.height as usize - bottom.len().min(area.height as usize);
    for (k, line) in bottom.iter().enumerate() {
        if footer_top + k < area.height as usize {
            buf.set_line(area.x, area.y + (footer_top + k) as u16, line, area.width);
        }
    }
    links
}

/// The buffer as plain text, one string per row with trailing spaces trimmed (for tests).
pub fn buffer_text(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(buf.area.x + x, buf.area.y + y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsky::{LinkCard, Profile};
    use crate::tui::feed::Msg;
    use crate::tui::theme::Theme;
    use chrono::{Duration as Cd, TimeZone, Utc};
    use ratatui::layout::Rect;
    use std::time::Duration;

    fn now() -> Now {
        Now {
            mono: std::time::Instant::now(),
            wall: Utc.with_ymd_and_hms(2026, 11, 4, 12, 0, 0).unwrap(),
        }
    }

    fn ago(n: Now, d: Cd) -> String {
        (n.wall - d).to_rfc3339()
    }

    fn post(rkey: &str, handle: &str, name: &str, text: &str, age: Cd) -> FeedItem {
        let mut it = FeedItem {
            uri: format!("at://did:{handle}/app.bsky.feed.post/{rkey}"),
            ..Default::default()
        };
        it.author = Profile {
            did: format!("did:{handle}"),
            handle: handle.into(),
            display_name: name.into(),
            ..Default::default()
        };
        it.post.text = text.into();
        it.post.created_at = ago(now(), age);
        it
    }

    fn app_with(items: Vec<FeedItem>, w: u16, h: u16) -> FeedApp {
        let mut a = FeedApp::new(
            Theme::named("dark", true, true),
            Duration::from_secs(30),
            false,
            "did:me",
            "me.test",
        );
        a.follow_loading = false;
        a.items = items;
        a.on_msg(Msg::Resize(w, h), now());
        a
    }

    fn draw(a: &FeedApp) -> (Vec<String>, Vec<LinkRegion>) {
        let mut buf = Buffer::empty(Rect::new(0, 0, a.width, a.height));
        let links = render(a, &mut buf, now());
        (buffer_text(&buf), links)
    }

    fn sample() -> Vec<FeedItem> {
        let n = now();
        let mut repost = post(
            "r1",
            "carol.test",
            "Carol",
            "A reposted thought that is long enough to need a second line when wrapped at this width",
            Cd::hours(5),
        );
        repost.by = "bob.test".into();
        repost.reposted_at = ago(n, Cd::minutes(8));
        let mut reply = post("p2", "dave.test", "", "replying here", Cd::minutes(3));
        reply.post.reply = true;
        reply.post.parent_handle = "alice.test".into();
        let mut quoted = post("q1", "erin.test", "Erin", "look at this", Cd::minutes(40));
        quoted.post.quote = "@frank.test: the quoted words\nacross lines".into();
        quoted.post.attach = vec!["▣ a cat".into()];
        quoted.post.link = Some(LinkCard {
            uri: "https://example.com/a".into(),
            title: "Example".into(),
        });
        quoted.like = "at://like".into();
        vec![
            post("p1", "alice.test", "Alice", "first post", Cd::minutes(1)),
            repost,
            reply,
            quoted,
        ]
    }

    #[test]
    fn list_layout_snapshot() {
        let a = app_with(sample(), 70, 24);
        let (rows, links) = draw(&a);
        let expected = [
            "bskyban   Following",
            "",
            "▸ 1m    Alice (@alice.test)",
            "        first post",
            "",
            "  8m    Carol (@carol.test) ↻ reposted by @bob.test · written 5h ago",
            "        A reposted thought that is long enough to need a second line",
            "        when wrapped at this width",
            "",
            "  3m    dave.test (@dave.test) ↳ replying to @alice.test",
            "        replying here",
            "",
            "  40m   Erin (@erin.test) ♥",
            "        look at this ▣ a cat ↗ Example https://example.com/a",
            "        ▎ @frank.test: the quoted words across lines",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "↑ ↓ move · PgUp PgDn page · Home End · R refresh · q quit",
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(rows[i], *want, "row {i}");
        }
        // every row's age is a link to the post on bsky.app, at the age's columns
        assert_eq!(links.len(), 4);
        assert_eq!(
            links[0],
            LinkRegion {
                x: 2,
                y: 2,
                len: 2,
                url: "https://bsky.app/profile/alice.test/post/p1".into()
            }
        );
        assert_eq!(links[1].url, "https://bsky.app/profile/carol.test/post/r1");
        assert_eq!((links[2].x, links[2].y, links[2].len), (2, 9, 2));
        assert_eq!((links[3].y, links[3].len), (12, 3));
    }

    #[test]
    fn selected_row_is_highlighted_across_its_lines_only() {
        let a = app_with(sample(), 70, 24);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 24));
        render(&a, &mut buf, now());
        let sel_bg = a.theme.sel_bg;
        for (y, expect) in [(2u16, true), (3, true), (4, false), (5, false)] {
            assert_eq!(buf[(40, y)].style().bg == sel_bg, expect, "row {y}");
        }
        // plain themes select with reverse video instead
        let mut b = app_with(sample(), 70, 24);
        b.theme = Theme::named("ansi", true, true);
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 24));
        render(&b, &mut buf, now());
        assert!(
            buf[(40, 2)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn new_post_indicator_and_refresh_warning_in_the_title() {
        let mut a = app_with(sample(), 70, 24);
        a.cur = 2;
        a.new_posts = vec![
            post("n1", "x.test", "X", "new", Cd::minutes(1)),
            post("n2", "y.test", "Y", "new", Cd::minutes(1)),
        ];
        assert_eq!(
            draw(&a).0[0],
            "bskyban   Following (+2 new)  R: jump to top"
        );
        a.refresh_err = Some("down".into());
        assert_eq!(
            draw(&a).0[0],
            "bskyban   Following (+2 new)  R: jump to top   ⚠ refresh failed"
        );
        a.new_posts.clear();
        assert_eq!(draw(&a).0[0], "bskyban   Following   ⚠ refresh failed");
    }

    #[test]
    fn footer_shows_error_status_and_loading_more() {
        let mut a = app_with(sample(), 70, 24);
        a.err = Some("refresh: boom\x1b[31m".into());
        a.status = "checking for new posts…".into();
        let rows = draw(&a).0;
        assert_eq!(
            &rows[21..],
            [
                "error: refresh: boom",
                "checking for new posts…",
                "↑ ↓ move · PgUp PgDn page · Home End · R refresh · q quit"
            ]
        );
        a.more_busy = true;
        assert_eq!(draw(&a).0[22], "loading more…");
    }

    #[test]
    fn empty_and_loading_states() {
        let mut a = app_with(vec![], 70, 24);
        assert_eq!(draw(&a).0[2], "Your timeline is empty.");
        a.follow_loading = true;
        a.spin_frame = 1;
        assert_eq!(draw(&a).0[2], "⣽ loading timeline…");
    }

    #[test]
    fn replies_to_the_post_above_are_indented_as_a_thread() {
        let mut parent = post("t0", "amy.test", "Amy", "the original", Cd::minutes(9));
        parent.cid = "c".into();
        let mut child = post("t1", "ben.test", "Ben", "an answer", Cd::minutes(2));
        child.post.reply = true;
        child.post.parent_handle = "amy.test".into();
        child.parent.uri = parent.uri.clone();
        let mut stranger = post("t2", "cy.test", "Cy", "another answer", Cd::minutes(1));
        stranger.post.reply = true;
        stranger.parent.uri = "at://did:other/app.bsky.feed.post/zz".into();
        let a = app_with(vec![parent, child, stranger], 70, 24);
        let (rows, links) = draw(&a);
        assert_eq!(
            rows[5],
            "      2m    Ben (@ben.test) ↳ replying to @amy.test"
        );
        assert_eq!(rows[6], "            an answer");
        assert_eq!(
            rows[8], "  1m    Cy (@cy.test) reply",
            "a reply to anything else is not indented"
        );
        assert_eq!(links[1].x, 6, "the link column follows the indent");
    }

    #[test]
    fn narrow_terminals_truncate_names_and_wrap_hints() {
        let a = app_with(
            vec![post(
                "p",
                "averyveryverylonghandle.example.com",
                "A Very Long Display Name Indeed",
                "hello",
                Cd::minutes(1),
            )],
            42,
            14,
        );
        let (rows, _) = draw(&a);
        assert!(rows[2].ends_with('…'), "{}", rows[2]);
        assert!(rows[2].chars().count() <= 42);
        assert_eq!(rows[12], "↑ ↓ move  PgUp PgDn page  Home End");
        assert_eq!(rows[13], "R refresh  q quit");
    }

    #[test]
    fn text_from_the_server_is_sanitized_before_it_reaches_the_screen() {
        let a = app_with(
            vec![post(
                "p",
                "evil.test",
                "Eve\x1b]0;x\x07",
                "hi \x1b[31mthere\x1b[0m\x07",
                Cd::minutes(1),
            )],
            70,
            24,
        );
        let (rows, _) = draw(&a);
        assert!(
            rows.iter()
                .all(|r| !r.contains('\x1b') && !r.contains('\x07'))
        );
        assert_eq!(rows[3], "        hi there");
    }

    #[test]
    fn decision_tags_come_from_saved_state() {
        let mut a = app_with(sample(), 70, 24);
        a.decisions.insert("did:alice.test".into(), "kept".into());
        a.decisions.insert("did:dave.test".into(), "blocked".into());
        let rows = draw(&a).0;
        assert_eq!(rows[2], "▸ 1m    Alice (@alice.test) kept");
        assert_eq!(
            rows[9],
            "  3m    dave.test (@dave.test) ↳ replying to @alice.test blocked"
        );
    }

    #[test]
    fn fancy_mode_marks_new_posts_while_they_fade() {
        let mut a = app_with(sample(), 70, 24);
        a.fancy = true;
        let t0 = now();
        a.fresh.insert(a.items[1].key(), t0.mono);
        let (rows, _) = draw(&a);
        assert!(
            rows[5].starts_with("┃ ") && rows[5].ends_with(" new"),
            "{}",
            rows[5]
        );
        assert!(rows[6].starts_with("┃ "), "text lines carry the bar too");
        assert!(!rows[9].starts_with('┃'));
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 24));
        render(&a, &mut buf, t0.plus(Duration::from_millis(1500)));
        assert_ne!(
            buf[(0, 5)].style().fg,
            a.theme.fresh.first().copied(),
            "colour has moved down the ramp"
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 70, 24));
        render(&a, &mut buf, t0.plus(Duration::from_secs(3)));
        assert_eq!(
            buf[(0, 5)].symbol(),
            " ",
            "highlight gone after two seconds"
        );
    }

    #[test]
    fn scrolled_list_only_shows_what_fits() {
        let items: Vec<FeedItem> = (0..30)
            .map(|i| {
                post(
                    &format!("p{i}"),
                    &format!("u{i}.test"),
                    &format!("User {i}"),
                    "text",
                    Cd::minutes(i + 1),
                )
            })
            .collect();
        let mut a = app_with(items, 60, 20);
        a.cur = 12;
        a.ensure_visible();
        let (rows, links) = draw(&a);
        assert!(a.list_top > 0);
        let selected = rows
            .iter()
            .position(|r| r.starts_with('▸'))
            .expect("selection visible");
        assert!(rows[selected].contains("User 12"));
        assert!(links.iter().all(|l| l.y < 20 - 2));
    }
}
