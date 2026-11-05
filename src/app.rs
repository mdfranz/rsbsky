//! Logic shared by the TUI and the subcommands: which repost authors still need a decision.

use std::collections::HashMap;

use crate::bsky::{Client, Error, FeedItem, Post, Profile};
use crate::store::{self, Audit, State};

/// A logged-in client with the local decisions it works against.
pub struct App {
    pub client: Client,
    pub state: State,
    pub audit: Audit,
}

/// How a candidate reached your timeline.
#[derive(Debug, Clone)]
pub struct RepostCtx {
    pub post: Post,
    /// The reposted post, for liking or replying to it.
    pub item: FeedItem,
    /// Handles of people you follow who reposted them, in first-seen order.
    pub by: Vec<String>,
}

impl App {
    /// Followers (newest first) you don't follow back and have no recorded decision for;
    /// plus skipped ones when `backlog`, or every follower when `all`. `since > 0` limits it
    /// to the newest N followers.
    pub async fn followers(
        &self,
        since: i64,
        all: bool,
        backlog: bool,
    ) -> Result<Vec<Profile>, Error> {
        let fs = self.client.get_followers(since).await?;
        if all {
            return Ok(fs);
        }
        Ok(fs.into_iter().filter(|f| self.is_new(f, backlog)).collect())
    }

    /// Whether `p` is someone you don't follow, haven't blocked, and have no recorded
    /// decision for (skipped ones count when `backlog` is true).
    pub fn is_new(&self, p: &Profile, backlog: bool) -> bool {
        if !p.viewer.following.is_empty()
            || !p.viewer.blocking.is_empty()
            || p.did == self.client.did
        {
            return false;
        }
        match self.state.entries.get(&p.did) {
            None => true,
            Some(e) => backlog && e.decision == store::SKIPPED,
        }
    }

    /// The original authors of reposts in your timeline that are new to you, in order of
    /// first appearance (newest first), each once.
    pub async fn feed_candidates(
        &self,
        pages: usize,
        backlog: bool,
    ) -> Result<(Vec<Profile>, HashMap<String, RepostCtx>), Error> {
        let reposts = self.client.get_timeline_reposts(pages).await?;
        let mut out = Vec::new();
        let mut rc: HashMap<String, RepostCtx> = HashMap::new();
        for r in reposts {
            if !self.is_new(&r.author, backlog) {
                continue;
            }
            let cur = rc.entry(r.author.did.clone()).or_insert_with(|| {
                out.push(r.author.clone());
                let mut post = r.post.clone();
                post.repost = false; // shown as the original post, under "Reposted by"
                RepostCtx {
                    post,
                    item: r.item.clone(),
                    by: Vec::new(),
                }
            });
            if !cur.by.contains(&r.by.handle) {
                cur.by.push(r.by.handle.clone());
            }
        }
        Ok((out, rc))
    }
}

/// Whether a decision leaves a record on your account (so that changing it later means
/// deleting that record).
pub fn is_remote(decision: &str) -> bool {
    decision == store::BLOCKED || decision == store::FOLLOWED
}

/// Performs the remote side of a block or follow decision; returns the new record's URI.
pub async fn create_for(c: &Client, decision: &str, did: &str) -> Result<String, Error> {
    if decision == store::FOLLOWED {
        c.follow(did).await
    } else {
        c.block(did).await
    }
}

/// Undoes the remote side of a block or follow decision.
pub async fn delete_for(c: &Client, decision: &str, uri: &str) -> Result<(), Error> {
    if decision == store::FOLLOWED {
        c.unfollow(uri).await
    } else {
        c.unblock(uri).await
    }
}

/// The record behind `decision`, as the server reports it for `p`.
pub fn viewer_uri<'a>(p: &'a Profile, decision: &str) -> &'a str {
    if decision == store::FOLLOWED {
        &p.viewer.following
    } else {
        &p.viewer.blocking
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Entry;

    fn app() -> App {
        let mut client = Client::new("http://unused");
        client.did = "did:me".into();
        let mut state = State::new("/nonexistent/state.json");
        for (did, d) in [("did:kept", "kept"), ("did:skip", "skipped")] {
            state.entries.insert(
                did.into(),
                Entry {
                    decision: d.into(),
                    ..Default::default()
                },
            );
        }
        App {
            client,
            state,
            audit: Audit::new("/nonexistent/audit.jsonl"),
        }
    }

    fn p(did: &str) -> Profile {
        Profile {
            did: did.into(),
            ..Default::default()
        }
    }

    #[test]
    fn is_new_rules() {
        let a = app();
        assert!(a.is_new(&p("did:fresh"), false));
        assert!(!a.is_new(&p("did:me"), false));
        assert!(!a.is_new(&p("did:kept"), true));
        assert!(!a.is_new(&p("did:skip"), false));
        assert!(a.is_new(&p("did:skip"), true));
        let mut f = p("did:f");
        f.viewer.following = "at://x".into();
        assert!(!a.is_new(&f, false));
        let mut b = p("did:b");
        b.viewer.blocking = "at://x".into();
        assert!(!a.is_new(&b, true));
    }
}
