//! `audit.jsonl`: an append-only JSON Lines log shared with the Go tool.
//! Field names, order and omission rules follow Go's `AuditEvent` (`omitempty` fields are
//! left out; `source` and `ok` are always written).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::bsky::nz;

/// What an account looked like when it was acted on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    #[serde(
        rename = "createdAt",
        skip_serializing_if = "String::is_empty",
        deserialize_with = "nz"
    )]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posts: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followers: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<i64>,
    #[serde(deserialize_with = "nz")]
    pub avatar: bool,
    /// First 200 characters.
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub bio: String,
}

impl Snapshot {
    /// What `p` looked like. `detailed` says whether `p` came from a full profile fetch:
    /// summaries (timeline authors) have no counts, so those are left out rather than
    /// logged as a misleading 0.
    pub fn of(p: &crate::bsky::Profile, detailed: bool) -> Snapshot {
        let bio: String = crate::termtext::collapse_ws(&p.description)
            .chars()
            .take(200)
            .collect();
        Snapshot {
            created_at: p.created_at.clone(),
            posts: detailed.then_some(p.posts_count),
            followers: detailed.then_some(p.followers_count),
            following: detailed.then_some(p.follows_count),
            avatar: !p.avatar.is_empty(),
            bio,
        }
    }
}

/// One line of the audit log.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditEvent {
    /// RFC 3339 UTC, filled in by [`Audit::log`] when empty.
    #[serde(deserialize_with = "nz")]
    pub ts: String,
    #[serde(deserialize_with = "nz")]
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub did: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub handle: String,
    /// cli, tui:review, tui:following, tui:reposts
    #[serde(deserialize_with = "nz")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub via: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub record: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub prev: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub detail: String,
    #[serde(
        rename = "dryRun",
        skip_serializing_if = "std::ops::Not::not",
        deserialize_with = "nz"
    )]
    pub dry_run: bool,
    #[serde(deserialize_with = "nz")]
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "nz")]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "nz")]
    pub snapshot: Option<Snapshot>,
}

/// Append-only audit log. Never rewrites or truncates the file.
#[derive(Debug, Clone)]
pub struct Audit {
    path: PathBuf,
}

impl Audit {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Audit { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `e` as one line. The directory is created 0700 and the file 0600.
    pub fn log(&self, mut e: AuditEvent) -> std::io::Result<()> {
        if e.ts.is_empty() {
            e.ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        }
        let mut line = go_json(serde_json::to_string(&e).map_err(std::io::Error::other)?);
        line.push('\n');
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)?;
        f.write_all(line.as_bytes())
    }
}

/// Every event in the log, oldest first. A missing file is an empty log. Blank lines are
/// skipped; a line that is not a JSON object fails the read with its line number.
pub fn read_audit(path: &Path) -> Result<Vec<AuditEvent>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let n = i + 1;
        let fail = |e: &dyn std::fmt::Display| format!("{} line {n}: {e}", path.display());
        if line.len() > 1 << 20 {
            return Err(fail(&"bufio.Scanner: token too long"));
        }
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        // Go accepts `null` (a zero event); arrays and scalars are errors, though serde
        // would happily read an array into a struct.
        let v: serde_json::Value = serde_json::from_slice(line).map_err(|e| fail(&e))?;
        match v {
            serde_json::Value::Null => out.push(AuditEvent::default()),
            serde_json::Value::Object(_) => {
                out.push(serde_json::from_value(v).map_err(|e| fail(&e))?)
            }
            _ => {
                return Err(fail(
                    &"json: cannot unmarshal into Go value of type store.AuditEvent",
                ));
            }
        }
    }
    Ok(out)
}

/// Go's `json.Marshal` escapes these characters even though JSON does not require it.
pub fn go_json(s: String) -> String {
    s.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn reads_what_it_writes_and_tolerates_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        assert!(
            read_audit(&path).unwrap().is_empty(),
            "a missing file is an empty log"
        );
        let a = Audit::new(&path);
        let ev = AuditEvent {
            action: "block".into(),
            did: "did:x".into(),
            handle: "x.test".into(),
            source: "tui:following".into(),
            via: "v.test".into(),
            record: "at://r".into(),
            prev: "kept".into(),
            detail: "d".into(),
            dry_run: true,
            ok: true,
            error: "e".into(),
            snapshot: Some(Snapshot {
                created_at: "2024".into(),
                posts: Some(0),
                followers: None,
                following: Some(3),
                avatar: true,
                bio: "b".into(),
            }),
            ..Default::default()
        };
        a.log(ev.clone()).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n  \nnull\n{\"action\":null,\"ok\":null,\"future\":1}\n")
            .unwrap();
        let got = read_audit(&path).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].snapshot, ev.snapshot);
        assert_eq!(
            AuditEvent {
                ts: String::new(),
                ..got[0].clone()
            },
            ev
        );
        assert_eq!(
            got[1],
            AuditEvent::default(),
            "null is a zero event, as in Go"
        );
        assert_eq!(got[2].action, "");
        std::fs::write(&path, "{\"action\":\"a\"}\n[]\n").unwrap();
        let err = read_audit(&path).unwrap_err();
        assert!(err.contains("audit.jsonl line 2:"), "{err}");
        std::fs::write(&path, "{\"action\":\"a\"}\n{oops\n").unwrap();
        assert!(read_audit(&path).unwrap_err().contains("line 2:"));
    }

    #[test]
    fn snapshot_leaves_counts_out_for_summaries() {
        let p = crate::bsky::Profile {
            description: "  hello   there ".into(),
            posts_count: 4,
            avatar: "x".into(),
            ..Default::default()
        };
        let full = Snapshot::of(&p, true);
        assert_eq!(
            (full.posts, full.bio.as_str(), full.avatar),
            (Some(4), "hello there", true)
        );
        let summary = Snapshot::of(&p, false);
        assert!(summary.posts.is_none() && summary.followers.is_none());
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("posts") && json.contains("\"avatar\":true"));
        let long = crate::bsky::Profile {
            description: "é".repeat(300),
            ..Default::default()
        };
        assert_eq!(Snapshot::of(&long, false).bio.chars().count(), 200);
    }

    #[test]
    fn writes_go_compatible_lines_with_private_modes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new/dir/audit.jsonl");
        let a = Audit::new(&path);
        a.log(AuditEvent {
            action: "refresh".into(),
            did: "did:me".into(),
            handle: "me.test".into(),
            source: "tui:following".into(),
            detail: "background check".into(),
            error: "boom <&>".into(),
            ..Default::default()
        })
        .unwrap();
        a.log(AuditEvent {
            action: "x".into(),
            source: "cli".into(),
            ok: true,
            ..Default::default()
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["action"], "refresh");
        assert_eq!(v["ok"], false, "ok is always written");
        assert_eq!(v["error"], "boom <&>");
        assert!(v.get("via").is_none() && v.get("dryRun").is_none() && v.get("snapshot").is_none());
        assert!(
            lines[0].starts_with("{\"ts\":\""),
            "ts comes first: {}",
            lines[0]
        );
        assert!(lines[0].contains("\\u003c"), "Go-style escaping");
        let ts = v["ts"].as_str().unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(ts).is_ok()
                && ts.ends_with('Z')
                && !ts.contains('.')
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
