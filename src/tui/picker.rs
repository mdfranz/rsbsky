//! The link picker (`o` / Enter) and opening links and images in the browser.

use std::process::{Command, Stdio};

use super::render::post_link;
use crate::bsky::{FeedItem, post_url};
use crate::termtext::{find_mentions, find_urls};

/// One link the picker offers for the selected post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickLink {
    /// What the picker shows.
    pub label: String,
    pub url: String,
}

/// Everything in `it` worth opening, in a fixed order: the post on bsky.app, its link card,
/// URLs in its text, the quoted post, mentioned accounts, then the author's profile.
/// Duplicates are dropped.
pub fn post_links(it: &FeedItem) -> Vec<PickLink> {
    let mut out: Vec<PickLink> = Vec::new();
    let mut add = |label: String, url: String| {
        if !url.is_empty() && !out.iter().any(|l| l.url == url) {
            out.push(PickLink { label, url });
        }
    };
    let p = &it.post;
    if !it.uri.is_empty() {
        add("post on bsky.app".into(), post_link(it));
    }
    if let Some(c) = &p.link {
        let label = if c.title.is_empty() { &c.uri } else { &c.title };
        add(format!("↗ {label}"), c.uri.clone());
    }
    for u in find_urls(&p.text) {
        // Go trims "https://" and then "http://" in sequence, once each
        let label = u.strip_prefix("https://").unwrap_or(&u);
        let label = label.strip_prefix("http://").unwrap_or(label).to_string();
        add(label, u);
    }
    if !p.quote_uri.is_empty() {
        let who = match p.quote.split_once(':') {
            Some((h, _)) if h.starts_with('@') => format!("{h}'s quoted post"),
            _ => "quoted post".to_string(),
        };
        add(who, post_url(&p.quote_uri));
    }
    for m in find_mentions(&p.text) {
        add(format!("@{m}"), format!("https://bsky.app/profile/{m}"));
    }
    if !it.author.handle.is_empty() {
        let h = &it.author.handle;
        add(
            format!("@{h}'s profile"),
            format!("https://bsky.app/profile/{h}"),
        );
    }
    out
}

/// How to open `url`: `$BROWSER` when set (the first of its colon-separated commands;
/// `%s` marks where the URL goes), otherwise `open` on macOS and `xdg-open` elsewhere.
pub fn browser_command(browser: &str, url: &str) -> Command {
    for b in browser.split(':') {
        let mut fields = b.split_whitespace();
        let Some(prog) = fields.next() else { continue };
        let mut args: Vec<String> = fields.map(String::from).collect();
        let mut placed = false;
        for a in &mut args {
            if a.contains("%s") {
                *a = a.replace("%s", url);
                placed = true;
            }
        }
        if !placed {
            args.push(url.to_string());
        }
        let mut c = Command::new(prog);
        c.args(args);
        return c;
    }
    let mut c = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    c.arg(url);
    c
}

/// Opens `url` without waiting for the browser. The launcher's stdio is detached so it
/// cannot scribble over the TUI, and it is reaped on a background thread.
pub fn open_url(url: &str) -> std::io::Result<()> {
    let browser = std::env::var("BROWSER").unwrap_or_default();
    let mut cmd = browser_command(&browser, url);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsky::{LinkCard, Profile};

    fn item() -> FeedItem {
        let mut it = FeedItem {
            uri: "at://did:a/app.bsky.feed.post/3k".into(),
            author: Profile {
                did: "did:a".into(),
                handle: "a.test".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        it.post.text =
            "see https://example.com/x. and https://example.com/x again, cc @bob.test @a.test"
                .into();
        it.post.link = Some(LinkCard {
            uri: "https://card.example/z".into(),
            title: "Card".into(),
        });
        it.post.quote = "@q.test: quoted".into();
        it.post.quote_uri = "at://did:q/app.bsky.feed.post/9".into();
        it
    }

    #[test]
    fn links_in_fixed_order_without_duplicates() {
        let got: Vec<(String, String)> = post_links(&item())
            .into_iter()
            .map(|l| (l.label, l.url))
            .collect();
        let want = [
            (
                "post on bsky.app",
                "https://bsky.app/profile/a.test/post/3k",
            ),
            ("↗ Card", "https://card.example/z"),
            ("example.com/x", "https://example.com/x"),
            (
                "@q.test's quoted post",
                "https://bsky.app/profile/did:q/post/9",
            ),
            ("@bob.test", "https://bsky.app/profile/bob.test"),
            ("@a.test", "https://bsky.app/profile/a.test"),
        ];
        assert_eq!(got, want.map(|(a, b)| (a.to_string(), b.to_string())));
        // the author's profile URL equals the @a.test mention's, so it is listed once
        assert!(!got.iter().any(|(l, _)| l == "@a.test's profile"));
    }

    #[test]
    fn card_without_title_uses_its_url_and_unknown_quote_author_is_generic() {
        let mut it = item();
        it.post.link = Some(LinkCard {
            uri: "https://c.example".into(),
            title: String::new(),
        });
        it.post.quote = "no colon".into();
        let links = post_links(&it);
        assert_eq!(links[1].label, "↗ https://c.example");
        assert!(links.iter().any(|l| l.label == "quoted post"));
        assert!(post_links(&FeedItem::default()).is_empty());
    }

    fn argv(c: &Command) -> Vec<String> {
        std::iter::once(c.get_program())
            .chain(c.get_args())
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn browser_env_wins_and_percent_s_marks_the_url() {
        assert_eq!(
            argv(&browser_command("firefox", "http://u")),
            ["firefox", "http://u"]
        );
        assert_eq!(
            argv(&browser_command("my-open --new %s --flag", "http://u")),
            ["my-open", "--new", "http://u", "--flag"]
        );
        assert_eq!(
            argv(&browser_command(":  :first-real:second", "http://u")),
            ["first-real", "http://u"]
        );
        let default = argv(&browser_command("", "http://u"));
        assert_eq!(default[1], "http://u");
        assert!(default[0] == "xdg-open" || default[0] == "open");
    }
}
