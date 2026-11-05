use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use super::client::{Client, Error};
use super::nz;
use crate::termtext::sanitize;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Viewer {
    #[serde(deserialize_with = "nz")]
    pub blocking: String,
    /// Set if you follow them (URI of your follow record).
    #[serde(deserialize_with = "nz")]
    pub following: String,
    /// Set if they follow you.
    #[serde(rename = "followedBy", deserialize_with = "nz")]
    pub followed_by: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawProfile {
    #[serde(deserialize_with = "nz")]
    did: String,
    #[serde(deserialize_with = "nz")]
    handle: String,
    #[serde(rename = "displayName", deserialize_with = "nz")]
    display_name: String,
    #[serde(deserialize_with = "nz")]
    description: String,
    #[serde(deserialize_with = "nz")]
    avatar: String,
    #[serde(rename = "createdAt", deserialize_with = "nz")]
    created_at: String,
    #[serde(rename = "followersCount", deserialize_with = "nz")]
    followers_count: i64,
    #[serde(rename = "followsCount", deserialize_with = "nz")]
    follows_count: i64,
    #[serde(rename = "postsCount", deserialize_with = "nz")]
    posts_count: i64,
    #[serde(deserialize_with = "nz")]
    viewer: Viewer,
}

/// The subset of `app.bsky.actor.defs#profileView(Detailed)` we use.
/// Text fields are sanitized when parsed, as in Go.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(from = "RawProfile")]
pub struct Profile {
    pub did: String,
    pub handle: String,
    pub display_name: String,
    pub description: String,
    pub avatar: String,
    pub created_at: String,
    pub followers_count: i64,
    pub follows_count: i64,
    pub posts_count: i64,
    pub viewer: Viewer,
}

impl From<RawProfile> for Profile {
    fn from(r: RawProfile) -> Self {
        Profile {
            did: sanitize(&r.did),
            handle: sanitize(&r.handle),
            display_name: sanitize(&r.display_name),
            description: sanitize(&r.description),
            avatar: r.avatar,
            created_at: r.created_at,
            followers_count: r.followers_count,
            follows_count: r.follows_count,
            posts_count: r.posts_count,
            viewer: r.viewer,
        }
    }
}

impl Profile {
    /// Account age, or zero if the creation time is unknown.
    pub fn age(&self) -> Duration {
        DateTime::parse_from_rfc3339(&self.created_at)
            .map(|t| Utc::now().signed_duration_since(t))
            .unwrap_or_else(|_| Duration::zero())
    }
}

impl Client {
    /// `actor` is a handle or DID.
    pub async fn get_profile(&self, actor: &str) -> Result<Profile, Error> {
        self.get("app.bsky.actor.getProfile", &[("actor", actor.to_string())])
            .await
    }

    /// Detailed profiles for DIDs/handles, fetched in batches of 25.
    pub async fn get_profiles(&self, actors: &[String]) -> Result<Vec<Profile>, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "nz")]
            profiles: Vec<Profile>,
        }
        let mut all = Vec::new();
        for chunk in actors.chunks(25) {
            let q: Vec<(&str, String)> = chunk.iter().map(|a| ("actors", a.clone())).collect();
            let out: Out = self.get("app.bsky.actor.getProfiles", &q).await?;
            all.extend(out.profiles);
        }
        Ok(all)
    }
}
