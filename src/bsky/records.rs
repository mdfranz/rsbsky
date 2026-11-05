//! Writing to your repo: block / follow records (and deleting them).

use serde::Deserialize;
use serde_json::json;

use super::client::{Client, Error};
use super::nz;

pub const BLOCK_COLLECTION: &str = "app.bsky.graph.block";
pub const FOLLOW_COLLECTION: &str = "app.bsky.graph.follow";

/// `2006-01-02T15:04:05Z`, as Go's `time.Now().UTC().Format(time.RFC3339)`.
fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

impl Client {
    /// Creates an `app.bsky.graph.<block|follow>` record about `did` in your repo and
    /// returns its AT URI.
    async fn create_subject_record(&self, collection: &str, did: &str) -> Result<String, Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(deserialize_with = "nz")]
            uri: String,
        }
        let out: Out = self
            .post(
                "com.atproto.repo.createRecord",
                &json!({
                    "repo": self.did,
                    "collection": collection,
                    "record": {"$type": collection, "subject": did, "createdAt": now_rfc3339()},
                }),
            )
            .await?;
        Ok(out.uri)
    }

    /// Removes the record at `uri` (`at://did/collection/rkey`) from your repo.
    async fn delete_record(&self, collection: &str, uri: &str) -> Result<(), Error> {
        let Some(i) = uri.rfind('/') else {
            return Err(Error::Api(format!("bad record uri {uri:?}")));
        };
        self.post_raw(
            "com.atproto.repo.deleteRecord",
            &json!({"repo": self.did, "collection": collection, "rkey": &uri[i + 1..]}),
        )
        .await
        .map(|_| ())
    }

    /// Creates a block record and returns its AT URI.
    pub async fn block(&self, did: &str) -> Result<String, Error> {
        self.create_subject_record(BLOCK_COLLECTION, did).await
    }

    /// Deletes the block record identified by its AT URI.
    pub async fn unblock(&self, block_uri: &str) -> Result<(), Error> {
        self.delete_record(BLOCK_COLLECTION, block_uri).await
    }

    /// Creates a follow record and returns its AT URI.
    pub async fn follow(&self, did: &str) -> Result<String, Error> {
        self.create_subject_record(FOLLOW_COLLECTION, did).await
    }

    /// Deletes the follow record identified by its AT URI.
    pub async fn unfollow(&self, follow_uri: &str) -> Result<(), Error> {
        self.delete_record(FOLLOW_COLLECTION, follow_uri).await
    }
}
