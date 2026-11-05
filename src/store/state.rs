//! `state.json`: `{"entries":{"<did>":{"handle":..,"decision":..,"at":..}}}`.
//! Phase 1 reads it with Go's leniency (null entries, missing fields); writing arrives in Phase 4.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

/// The decision recorded for one account.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub handle: String,
    pub decision: String,
    pub at: DateTime<FixedOffset>,
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
    fn malformed() {
        assert!(load("").is_err());
        assert!(load("[]").is_err());
        assert!(load("{").is_err());
        assert!(load(r#"{"entries":{"d":{"at":"yesterday"}}}"#).is_err());
        assert!(load(r#"{"entries":[]}"#).is_err());
    }
}
