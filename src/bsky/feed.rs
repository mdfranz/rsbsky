//! Timeline and author-feed requests, and the `feedViewPost` model.

use serde::Deserialize;
use serde_json::Value;

use super::client::{Client, Error};
use super::nz;
use super::post::{Facet, Post, describe_embed, embed_link, expand_links, view_context};
use super::profile::Profile;

#[derive(Deserialize, Default)]
#[serde(default)]
struct FeedEntry {
    #[serde(deserialize_with = "nz")]
    post: PostView,
    #[serde(deserialize_with = "nz")]
    reply: ReplyView,
    #[serde(deserialize_with = "nz")]
    reason: Reason,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PostView {
    #[serde(deserialize_with = "nz")]
    uri: String,
    #[serde(deserialize_with = "nz")]
    cid: String,
    #[serde(rename = "likeCount", deserialize_with = "nz")]
    like_count: i64,
    #[serde(rename = "repostCount", deserialize_with = "nz")]
    repost_count: i64,
    #[serde(rename = "replyCount", deserialize_with = "nz")]
    reply_count: i64,
    #[serde(rename = "quoteCount", deserialize_with = "nz")]
    quote_count: i64,
    #[serde(deserialize_with = "nz")]
    viewer: PostViewer,
    #[serde(deserialize_with = "nz")]
    author: Profile,
    /// Hydrated embed view.
    embed: Option<Value>,
    #[serde(deserialize_with = "nz")]
    record: Record,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PostViewer {
    #[serde(deserialize_with = "nz")]
    like: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Record {
    #[serde(deserialize_with = "nz")]
    text: String,
    #[serde(rename = "createdAt", deserialize_with = "nz")]
    created_at: String,
    reply: Option<Value>,
    embed: Option<Value>,
    #[serde(deserialize_with = "nz")]
    facets: Vec<Facet>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyView {
    #[serde(deserialize_with = "nz")]
    parent: ParentView,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParentView {
    #[serde(deserialize_with = "nz")]
    author: Profile,
    #[serde(deserialize_with = "nz")]
    record: ParentRecord,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParentRecord {
    #[serde(deserialize_with = "nz")]
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Reason {
    #[serde(rename = "$type", deserialize_with = "nz")]
    ty: String,
    #[serde(deserialize_with = "nz")]
    by: Profile,
    /// When it was reposted.
    #[serde(rename = "indexedAt", deserialize_with = "nz")]
    indexed_at: String,
}

/// A strong reference to a post: AT URI plus content hash.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PostRef {
    #[serde(deserialize_with = "nz")]
    pub uri: String,
    #[serde(deserialize_with = "nz")]
    pub cid: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyRef {
    #[serde(deserialize_with = "nz")]
    root: PostRef,
    #[serde(deserialize_with = "nz")]
    parent: PostRef,
}

/// A post's engagement counts as the server reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostStats {
    pub likes: i64,
    pub reposts: i64,
    pub replies: i64,
    pub quotes: i64,
}

/// One timeline entry: a post, its author, and who reposted it (if anyone).
#[derive(Debug, Clone, Default)]
pub struct FeedItem {
    pub post: Post,
    pub author: Profile,
    /// Handle of the reposter, empty for original posts.
    pub by: String,
    pub uri: String,
    pub cid: String,
    /// Thread root when the post is a reply.
    pub root: PostRef,
    /// The post it answers, when it is a reply.
    pub parent: PostRef,
    /// AT URI of your like of this post, if you liked it.
    pub like: String,
    /// When `by` reposted it (RFC 3339).
    pub reposted_at: String,
    pub stats: PostStats,
}

impl FeedItem {
    /// Identifies a timeline entry: the same post can appear once per reposter.
    pub fn key(&self) -> String {
        format!("{}|{}", self.uri, self.by)
    }
}

/// A post someone you follow reposted, with its original author.
#[derive(Debug, Clone)]
pub struct Repost {
    /// The reposted post itself, for liking or replying to it.
    pub item: FeedItem,
    pub post: Post,
    /// Original author.
    pub author: Profile,
    /// Who reposted it.
    pub by: Profile,
}

impl FeedEntry {
    fn is_repost(&self) -> bool {
        self.reason.ty.ends_with("reasonRepost")
    }

    fn to_post(&self) -> Post {
        let r = &self.post.record;
        let mut p = Post {
            text: expand_links(&r.text, &r.facets),
            created_at: r.created_at.clone(),
            repost: self.is_repost(),
            reply: r.reply.is_some(),
            attach: describe_embed(&r.embed),
            link: embed_link(&r.embed),
            parent_handle: self.reply.parent.author.handle.clone(),
            parent_text: self.reply.parent.record.text.clone(),
            ..Default::default()
        };
        (p.quote, p.quote_uri, p.images) = view_context(&self.post.embed);
        if !p.quote.is_empty() {
            // the quote text replaces the bare "[quote post]" marker
            p.attach.retain(|s| s != "[quote post]");
        }
        p
    }

    fn item(&self) -> FeedItem {
        let refs: ReplyRef = self
            .post
            .record
            .reply
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let p = &self.post;
        FeedItem {
            post: self.to_post(),
            author: p.author.clone(),
            by: String::new(),
            uri: p.uri.clone(),
            cid: p.cid.clone(),
            root: refs.root,
            parent: refs.parent,
            like: p.viewer.like.clone(),
            reposted_at: String::new(),
            stats: PostStats {
                likes: p.like_count,
                reposts: p.repost_count,
                replies: p.reply_count,
                quotes: p.quote_count,
            },
        }
    }
}

impl Client {
    async fn feed_page(
        &self,
        nsid: &str,
        q: &[(&str, String)],
    ) -> Result<(Vec<FeedEntry>, String), Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "nz")]
            feed: Vec<FeedEntry>,
            #[serde(deserialize_with = "nz")]
            cursor: String,
        }
        let out: Out = self.get(nsid, q).await?;
        Ok((out.feed, out.cursor))
    }

    /// One page of your home timeline (accounts you follow), newest first, plus the
    /// cursor for the next page (empty when there is none).
    pub async fn get_timeline(
        &self,
        cursor: &str,
        limit: usize,
    ) -> Result<(Vec<FeedItem>, String), Error> {
        let mut q = vec![("limit", limit.to_string())];
        if !cursor.is_empty() {
            q.push(("cursor", cursor.to_string()));
        }
        let (entries, next) = self.feed_page("app.bsky.feed.getTimeline", &q).await?;
        let items = entries
            .iter()
            .map(|e| {
                let mut it = e.item();
                if e.is_repost() {
                    it.by = e.reason.by.handle.clone();
                    it.reposted_at = e.reason.indexed_at.clone();
                    it.post.repost = false; // this is the original post; `by` says who reposted it
                }
                it
            })
            .collect();
        Ok((items, next))
    }

    /// Scans up to `pages` pages (100 items each, at least one) of your home timeline,
    /// newest first, and returns the reposts in it.
    pub async fn get_timeline_reposts(&self, pages: usize) -> Result<Vec<Repost>, Error> {
        let mut reposts = Vec::new();
        let mut cursor = String::new();
        for _ in 0..pages.max(1) {
            let mut q = vec![("limit", "100".to_string())];
            if !cursor.is_empty() {
                q.push(("cursor", cursor.clone()));
            }
            let (entries, next) = self.feed_page("app.bsky.feed.getTimeline", &q).await?;
            for e in &entries {
                if e.is_repost() {
                    reposts.push(Repost {
                        item: e.item(),
                        post: e.to_post(),
                        author: e.post.author.clone(),
                        by: e.reason.by.clone(),
                    });
                }
            }
            if next.is_empty() || entries.is_empty() {
                break;
            }
            cursor = next;
        }
        Ok(reposts)
    }
}
