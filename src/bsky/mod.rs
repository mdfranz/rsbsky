//! Bluesky XRPC client and the data model parsed from its responses.

pub mod client;
pub mod feed;
pub mod post;
pub mod profile;
pub mod records;

pub use client::{Client, Error};
pub use feed::{FeedItem, PostRef, PostStats, Repost};
pub use post::{LinkCard, Post};
pub use profile::{Profile, Viewer};

use serde::{Deserialize, Deserializer};

/// Go's JSON decoding treats `null` like a missing field; so do we.
pub fn nz<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Turns `at://did/app.bsky.feed.post/rkey` into its bsky.app link.
pub fn post_url(uri: &str) -> String {
    if let (Some(i), Some(rest)) = (uri.rfind('/'), uri.strip_prefix("at://")) {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() == 3 {
            return format!(
                "https://bsky.app/profile/{}/post/{}",
                parts[0],
                &uri[i + 1..]
            );
        }
    }
    uri.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            post_url("at://did:plc:x/app.bsky.feed.post/3k"),
            "https://bsky.app/profile/did:plc:x/post/3k"
        );
        assert_eq!(post_url("at://x/y"), "at://x/y");
        assert_eq!(post_url("other"), "other");
    }
}
