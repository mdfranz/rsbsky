//! `state.json`: `{"entries":{"<did>":{"handle":..,"decision":..,"at":..}}}`.
//! Reading has Go's leniency (null entries, missing fields); writing is atomic (tmp + rename),
//! creates the directory 0700 and the file 0600, and in-memory changes roll back when the
//! save fails.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};

/// The decision recorded for one account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub handle: String,
    pub decision: String,
    #[serde(serialize_with = "serialize_time")]
    pub at: DateTime<FixedOffset>,
}

/// Go's `time.RFC3339Nano`: fractional seconds only when non-zero, trailing zeros trimmed,
/// `Z` for UTC.
pub fn go_time(t: &DateTime<FixedOffset>) -> String {
    let mut s = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = t.nanosecond() % 1_000_000_000;
    if nanos > 0 {
        s.push('.');
        s.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    if t.offset().local_minus_utc() == 0 {
        s.push('Z');
    } else {
        s.push_str(&t.format("%:z").to_string());
    }
    s
}

fn serialize_time<S: serde::Serializer>(
    t: &DateTime<FixedOffset>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&go_time(t))
}

impl Entry {
    /// An entry stamped with the current time.
    pub fn now(handle: &str, decision: &str) -> Entry {
        Entry {
            handle: handle.into(),
            decision: decision.into(),
            at: Utc::now().fixed_offset(),
        }
    }
}

/// Go's zero `time.Time` (0001-01-01T00:00:00Z).
fn zero_time() -> DateTime<FixedOffset> {
    let d = NaiveDate::from_ymd_opt(1, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    Utc.from_utc_datetime(&d).fixed_offset()
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            handle: String::new(),
            decision: String::new(),
            at: zero_time(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEntry {
    handle: Option<String>,
    decision: Option<String>,
    at: Option<String>,
}

fn object_or_null(
    v: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, StateError> {
    match v {
        serde_json::Value::Object(m) => Ok(m),
        serde_json::Value::Null => Ok(Default::default()),
        other => Err(StateError::Shape(match other {
            serde_json::Value::Array(_) => "array",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Number(_) => "number",
            _ => "bool",
        })),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("json: cannot unmarshal {0} into Go value of type object")]
    Shape(&'static str),
    #[error("parsing time {0:?} as RFC 3339")]
    Time(String),
}

/// Accounts (by DID) mapped to the decision made about them.
#[derive(Debug, Clone)]
pub struct State {
    path: PathBuf,
    pub entries: BTreeMap<String, Entry>,
}

impl State {
    /// An empty state that would save to `path`. Nothing is read or written.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        State {
            path: path.into(),
            entries: BTreeMap::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads `path`; a missing file is an empty state.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StateError> {
        let mut s = State::new(path);
        let bytes = match std::fs::read(&s.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(s),
            Err(e) => return Err(e.into()),
        };
        // Go accepts `null` anywhere an object is expected and treats it as empty; arrays
        // and scalars are errors (serde would otherwise accept an array for a struct).
        let root: serde_json::Value = serde_json::from_slice(&bytes)?;
        let top = object_or_null(root)?;
        let entries = match top.get("entries").cloned() {
            None | Some(serde_json::Value::Null) => Default::default(),
            Some(v) => object_or_null(v)?,
        };
        for (did, v) in entries {
            let e: RawEntry =
                serde_json::from_value(serde_json::Value::Object(object_or_null(v)?))?;
            let at = match e.at {
                None => zero_time(),
                Some(t) => DateTime::parse_from_rfc3339(&t).map_err(|_| StateError::Time(t))?,
            };
            s.entries.insert(
                did,
                Entry {
                    handle: e.handle.unwrap_or_default(),
                    decision: e.decision.unwrap_or_default(),
                    at,
                },
            );
        }
        Ok(s)
    }

    /// Writes the file atomically. Creates the directory 0700 and the file 0600.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        #[derive(Serialize)]
        struct File<'a> {
            entries: &'a BTreeMap<String, Entry>,
        }
        let json = serde_json::to_string_pretty(&File {
            entries: &self.entries,
        })
        .map_err(std::io::Error::other)?;
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(super::audit::go_json(json).as_bytes())?;
        drop(f);
        std::fs::rename(&tmp, &self.path)
    }

    /// Records `decision` for `did` as of now.
    pub fn set(&mut self, did: &str, handle: &str, decision: &str) -> std::io::Result<()> {
        self.put(did, Entry::now(handle, decision))
    }

    /// Stores `e` as-is (used to restore a previous decision). Rolls back if saving fails.
    pub fn put(&mut self, did: &str, e: Entry) -> std::io::Result<()> {
        let prev = self.entries.insert(did.to_string(), e);
        if let Err(err) = self.save() {
            match prev {
                Some(p) => self.entries.insert(did.to_string(), p),
                None => self.entries.remove(did),
            };
            return Err(err);
        }
        Ok(())
    }

    /// Forgets the decision for `did`. Rolls back if saving fails.
    pub fn delete(&mut self, did: &str) -> std::io::Result<()> {
        let prev = self.entries.remove(did);
        if let Err(err) = self.save() {
            if let Some(p) = prev {
                self.entries.insert(did.to_string(), p);
            }
            return Err(err);
        }
        Ok(())
    }

    pub fn decision(&self, did: &str) -> &str {
        self.entries.get(did).map_or("", |e| e.decision.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(json: &str) -> Result<State, StateError> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        std::fs::write(&p, json).unwrap();
        State::load(p)
    }

    #[test]
    fn missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            State::load(dir.path().join("none.json"))
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn lenient_shapes() {
        assert!(load("null").unwrap().entries.is_empty());
        assert!(load("{}").unwrap().entries.is_empty());
        assert!(load(r#"{"entries":null}"#).unwrap().entries.is_empty());
        let s = load(
            r#"{"entries":{"did:a":null,"did:b":{"handle":"b","decision":"kept","at":"2024-05-01T10:00:00.123456789+02:00"},"did:c":{"decision":null}}}"#,
        )
        .unwrap();
        assert_eq!(s.entries["did:a"], Entry::default());
        assert_eq!(s.decision("did:b"), "kept");
        assert_eq!(s.entries["did:b"].at.timestamp_subsec_nanos(), 123_456_789);
        assert_eq!(s.decision("did:c"), "");
        assert_eq!(s.decision("did:zzz"), "");
    }

    #[test]
    fn go_time_formats_like_rfc3339nano() {
        let p = |s: &str| DateTime::parse_from_rfc3339(s).unwrap();
        assert_eq!(go_time(&p("2024-05-01T10:00:00Z")), "2024-05-01T10:00:00Z");
        assert_eq!(
            go_time(&p("2024-05-01T10:00:00.120Z")),
            "2024-05-01T10:00:00.12Z"
        );
        assert_eq!(
            go_time(&p("2024-05-01T10:00:00.123456789+02:00")),
            "2024-05-01T10:00:00.123456789+02:00"
        );
        assert_eq!(
            go_time(&p("2024-05-01T10:00:00-05:30")),
            "2024-05-01T10:00:00-05:30"
        );
        assert_eq!(go_time(&zero_time()), "0001-01-01T00:00:00Z");
    }

    #[test]
    fn save_round_trips_and_preserves_timestamps_and_odd_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"entries":{"did:n":null,"did:z":{"handle":"z","decision":"kept"},"did:u":{"handle":"ü<&>","decision":"blocked","at":"2024-05-01T10:00:00.5+02:00"}}}"#,
        )
        .unwrap();
        let mut s = State::load(&path).unwrap();
        s.set("did:new", "new.test", "followed").unwrap();
        let again = State::load(&path).unwrap();
        assert_eq!(again.entries, s.entries);
        assert_eq!(again.entries["did:n"], Entry::default());
        assert_eq!(
            go_time(&again.entries["did:z"].at),
            "0001-01-01T00:00:00Z",
            "a missing time round-trips as Go's zero time"
        );
        assert_eq!(
            go_time(&again.entries["did:u"].at),
            "2024-05-01T10:00:00.5+02:00"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("{\n  \"entries\": {\n    \"did:n\""),
            "2-space indent, sorted keys: {text}"
        );
        assert!(text.contains("\\u003c\\u0026\\u003e"), "Go-style escaping");
        assert!(
            !path.with_extension("json.tmp").exists(),
            "temp file is renamed away"
        );
    }

    #[test]
    fn save_creates_private_directory_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new/state.json");
        let mut s = State::new(&path);
        s.set("did:a", "a", "kept").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn failed_saves_roll_back_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = State::new(&path);
        s.set("did:a", "a", "kept").unwrap();
        // make the save fail: the temp file path is now a directory
        std::fs::create_dir(dir.path().join("state.json.tmp")).unwrap();
        assert!(s.set("did:b", "b", "blocked").is_err());
        assert!(!s.entries.contains_key("did:b"), "new entry rolled back");
        let before = s.entries["did:a"].clone();
        assert!(s.set("did:a", "a", "blocked").is_err());
        assert_eq!(s.entries["did:a"], before, "replaced entry restored");
        assert!(s.delete("did:a").is_err());
        assert_eq!(s.entries["did:a"], before, "deleted entry restored");
        assert!(s.delete("did:missing").is_err() && !s.entries.contains_key("did:missing"));
        std::fs::remove_dir(dir.path().join("state.json.tmp")).unwrap();
        s.delete("did:a").unwrap();
        assert!(State::load(&path).unwrap().entries.is_empty());
    }

    #[test]
    fn malformed() {
        assert!(load("").is_err());
        assert!(load("[]").is_err());
        assert!(load("{").is_err());
        assert!(load(r#"{"entries":{"d":{"at":"yesterday"}}}"#).is_err());
        assert!(load(r#"{"entries":[]}"#).is_err());
    }
}
