//! Post model and embed parsing (images, video, external link cards, quotes).

use serde::Deserialize;
use serde_json::Value;

use super::nz;
use super::profile::Profile;
use crate::termtext::{IMAGE_MARK, VIDEO_MARK, collapse_ws};

/// A post (or repost) in an account's feed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Post {
    pub text: String,
    pub created_at: String,
    pub repost: bool,
    pub reply: bool,
    /// One-line descriptions of images, video, quotes.
    pub attach: Vec<String>,
    /// Link preview card, if the post has one.
    pub link: Option<LinkCard>,

    // Only set for posts that come with hydrated context (timeline / author feeds).
    /// `@handle: text` of a quoted post.
    pub quote: String,
    pub quote_uri: String,
    /// Full-size image URLs.
    pub images: Vec<String>,
    /// For replies: who is being replied to.
    pub parent_handle: String,
    pub parent_text: String,
}

/// A post's link preview: where it goes and the page title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCard {
    pub uri: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Facet {
    #[serde(deserialize_with = "nz")]
    pub index: FacetIndex,
    #[serde(deserialize_with = "nz")]
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FacetIndex {
    #[serde(rename = "byteStart", deserialize_with = "nz")]
    pub byte_start: i64,
    #[serde(rename = "byteEnd", deserialize_with = "nz")]
    pub byte_end: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Feature {
    #[serde(rename = "$type", deserialize_with = "nz")]
    pub ty: String,
    #[serde(deserialize_with = "nz")]
    pub uri: String,
}

/// Replaces the shortened link text clients store in a post
/// (`www.example.com/some-pa...`) with the real URL from the link facet.
/// Offsets are UTF-8 byte offsets; a bad offset leaves the text unchanged.
pub fn expand_links(text: &str, facets: &[Facet]) -> String {
    let original_len = text.len() as i64;
    let mut reps: Vec<(usize, usize, &str)> = Vec::new();
    for f in facets {
        for ft in &f.features {
            let (s, e) = (f.index.byte_start, f.index.byte_end);
            if ft.ty.ends_with("#link")
                && !ft.uri.is_empty()
                && s >= 0
                && s < e
                && e <= original_len
            {
                reps.push((s as usize, e as usize, &ft.uri));
            }
        }
    }
    reps.sort_by_key(|r| std::cmp::Reverse(r.0)); // back to front so earlier offsets stay valid
    let mut bytes = text.as_bytes().to_vec();
    for (s, e, uri) in reps {
        if e > bytes.len() {
            continue;
        }
        let mut next = Vec::with_capacity(bytes.len() + uri.len());
        next.extend_from_slice(&bytes[..s]);
        next.extend_from_slice(uri.as_bytes());
        next.extend_from_slice(&bytes[e..]);
        bytes = next;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct External {
    #[serde(deserialize_with = "nz")]
    uri: String,
    #[serde(deserialize_with = "nz")]
    title: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Embed {
    #[serde(rename = "$type", deserialize_with = "nz")]
    ty: String,
    #[serde(deserialize_with = "nz")]
    images: Vec<Image>,
    #[serde(deserialize_with = "nz")]
    alt: String,
    #[serde(deserialize_with = "nz")]
    external: External,
    media: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Image {
    #[serde(deserialize_with = "nz")]
    alt: String,
    #[serde(deserialize_with = "nz")]
    fullsize: String,
}

fn parse<T: for<'de> Deserialize<'de>>(v: &Option<Value>) -> Option<T> {
    v.as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The post's link card, if its embed has one.
pub fn embed_link(raw: &Option<Value>) -> Option<LinkCard> {
    let e: Embed = parse(raw)?;
    if e.ty.starts_with("app.bsky.embed.external") && !e.external.uri.is_empty() {
        return Some(LinkCard {
            uri: e.external.uri,
            title: collapse_ws(&e.external.title),
        });
    }
    if e.ty == "app.bsky.embed.recordWithMedia" {
        return embed_link(&e.media);
    }
    None
}

/// Short one-line descriptions of a raw post embed.
pub fn describe_embed(raw: &Option<Value>) -> Vec<String> {
    let Some(e) = parse::<Embed>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if e.ty.starts_with("app.bsky.embed.images") {
        // "▣", or "▣×3" for several, then the first alt text
        let mut s = IMAGE_MARK.to_string();
        if e.images.len() != 1 {
            s.push_str(&format!("×{}", e.images.len()));
        }
        if let Some(alt) = e
            .images
            .iter()
            .map(|i| collapse_ws(&i.alt))
            .find(|a| !a.is_empty())
        {
            s.push(' ');
            s.push_str(&alt);
        }
        out.push(s);
    } else if e.ty.starts_with("app.bsky.embed.video") {
        let mut s = format!("{VIDEO_MARK} video");
        let alt = collapse_ws(&e.alt);
        if !alt.is_empty() {
            s.push(' ');
            s.push_str(&alt);
        }
        out.push(s);
    } else if e.ty == "app.bsky.embed.recordWithMedia" {
        out.extend(describe_embed(&e.media));
        out.push("[quote post]".into());
    } else if e.ty.starts_with("app.bsky.embed.record") {
        out.push("[quote post]".into());
    }
    out
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ViewEmbed {
    #[serde(deserialize_with = "nz")]
    images: Vec<Image>,
    record: Option<Value>,
    media: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QuoteRecord {
    #[serde(deserialize_with = "nz")]
    uri: String,
    #[serde(deserialize_with = "nz")]
    author: Profile,
    #[serde(deserialize_with = "nz")]
    value: QuoteValue,
    record: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QuoteValue {
    #[serde(deserialize_with = "nz")]
    text: String,
    #[serde(deserialize_with = "nz")]
    facets: Vec<Facet>,
}

/// Pulls a quoted post (`@handle: text`), its URI and image URLs out of a hydrated embed view.
pub fn view_context(raw: &Option<Value>) -> (String, String, Vec<String>) {
    let Some(v) = parse::<ViewEmbed>(raw) else {
        return Default::default();
    };
    let (mut quote, mut quote_uri) = (String::new(), String::new());
    let mut images: Vec<String> = v
        .images
        .iter()
        .filter(|i| !i.fullsize.is_empty())
        .map(|i| i.fullsize.clone())
        .collect();
    if v.media.is_some() {
        // recordWithMedia
        images.extend(view_context(&v.media).2);
    }
    if let Some(mut r) = parse::<QuoteRecord>(&v.record) {
        if r.author.handle.is_empty() && r.record.is_some() {
            // recordWithMedia nests the quoted record one level deeper
            if let Some(inner) = parse::<QuoteRecord>(&r.record) {
                let uri = if inner.uri.is_empty() {
                    r.uri.clone()
                } else {
                    inner.uri.clone()
                };
                r = QuoteRecord { uri, ..inner };
            }
        }
        if !r.author.handle.is_empty() {
            quote = format!(
                "@{}: {}",
                r.author.handle,
                expand_links(&r.value.text, &r.value.facets)
            );
            quote_uri = r.uri;
        }
    }
    (quote, quote_uri, images)
}

impl LinkCard {
    fn title_for(&self, text: &str) -> &str {
        if self.title.is_empty() || text.to_lowercase().contains(&self.title.to_lowercase()) {
            ""
        } else {
            &self.title
        }
    }

    /// The card in one line: `↗ Title https://…`. The URL is left out when the post's text
    /// already has it, and the title when the text already says it.
    pub fn label(&self, text: &str) -> String {
        let mut parts = vec!["↗"];
        let t = self.title_for(text);
        if !t.is_empty() {
            parts.push(t);
        }
        if !text.contains(&self.uri) {
            parts.push(&self.uri);
        }
        if parts.len() == 1 {
            String::new()
        } else {
            parts.join(" ")
        }
    }
}

impl Post {
    /// The post's text, attachments and link card as one paragraph.
    pub fn preview(&self) -> String {
        let mut parts = vec![self.text.clone()];
        parts.extend(self.attach.iter().cloned());
        if let Some(l) = &self.link {
            parts.push(l.label(&self.text));
        }
        collapse_ws(&parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facet(s: i64, e: i64, uri: &str) -> Facet {
        serde_json::from_value(json!({"index":{"byteStart":s,"byteEnd":e},
            "features":[{"$type":"app.bsky.richtext.facet#link","uri":uri}]}))
        .unwrap()
    }

    #[test]
    fn links_expand_by_byte_offsets() {
        // "é" is two bytes, so the link starts at byte 5
        let t = "café exa… end";
        let start = t.find("exa…").unwrap() as i64;
        let end = start + "exa…".len() as i64;
        assert_eq!(
            expand_links(t, &[facet(start, end, "https://example.com/x")]),
            "café https://example.com/x end"
        );
        assert_eq!(expand_links(t, &[facet(0, 999, "u")]), t);
        assert_eq!(expand_links("ab", &[facet(1, 1, "u")]), "ab");
    }

    #[test]
    fn embeds() {
        let img = Some(
            json!({"$type":"app.bsky.embed.images","images":[{"alt":" a  cat "},{"alt":"b"}]}),
        );
        assert_eq!(describe_embed(&img), vec!["▣×2 a cat"]);
        let one = Some(json!({"$type":"app.bsky.embed.images","images":[{"alt":""}]}));
        assert_eq!(describe_embed(&one), vec!["▣"]);
        let vid = Some(json!({"$type":"app.bsky.embed.video","alt":"clip"}));
        assert_eq!(describe_embed(&vid), vec!["▶ video clip"]);
        let rwm = Some(json!({"$type":"app.bsky.embed.recordWithMedia",
            "media":{"$type":"app.bsky.embed.external","external":{"uri":"https://e.com","title":" T  x "}}}));
        assert_eq!(
            embed_link(&rwm),
            Some(LinkCard {
                uri: "https://e.com".into(),
                title: "T x".into()
            })
        );
        assert_eq!(describe_embed(&rwm), vec!["[quote post]"]);
        assert!(describe_embed(&None).is_empty());
    }

    #[test]
    fn quoted_post_context() {
        let direct = Some(json!({"$type":"app.bsky.embed.record#view","record":{
            "uri":"at://d/app.bsky.feed.post/1","author":{"handle":"q.test"},"value":{"text":"hi"}}}));
        let (q, u, _) = view_context(&direct);
        assert_eq!(
            (q.as_str(), u.as_str()),
            ("@q.test: hi", "at://d/app.bsky.feed.post/1")
        );
        let nested = Some(json!({"record":{"record":{
            "uri":"at://d/app.bsky.feed.post/2","author":{"handle":"n.test"},"value":{"text":"yo"}}},
            "media":{"images":[{"fullsize":"https://img/1"}]}}));
        let (q, u, imgs) = view_context(&nested);
        assert_eq!(
            (q.as_str(), u.as_str()),
            ("@n.test: yo", "at://d/app.bsky.feed.post/2")
        );
        assert_eq!(imgs, vec!["https://img/1"]);
        let blocked = Some(json!({"record":{"uri":"x","author":{"did":"did:x"}}}));
        assert_eq!(view_context(&blocked).0, "");
    }

    #[test]
    fn link_label() {
        let c = LinkCard {
            uri: "https://e.com".into(),
            title: "Hello".into(),
        };
        assert_eq!(c.label("nothing"), "↗ Hello https://e.com");
        assert_eq!(c.label("see https://e.com"), "↗ Hello");
        assert_eq!(c.label("hello https://e.com"), "");
    }
}
