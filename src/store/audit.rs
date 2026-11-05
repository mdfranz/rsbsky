//! `audit.jsonl`: an append-only JSON Lines log shared with the Go tool.
//! Field names, order and omission rules follow Go's `AuditEvent` (`omitempty` fields are
//! left out; `source` and `ok` are always written).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;

/// What an account looked like when it was acted on.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Snapshot {
    #[serde(rename = "createdAt", skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posts: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followers: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<i64>,
    pub avatar: bool,
    /// First 200 characters.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bio: String,
}

/// One line of the audit log.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AuditEvent {
    /// RFC 3339 UTC, filled in by [`Audit::log`] when empty.
    pub ts: String,
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub did: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub handle: String,
    /// cli, tui:review, tui:following, tui:reposts
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub via: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub record: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prev: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(rename = "dryRun", skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
