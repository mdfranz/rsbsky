//! Local files written by bskyban: `state.json` (decisions) and `audit.jsonl`.
//! The formats are shared with the Go tool.

pub mod audit;
pub mod state;

use std::path::{Path, PathBuf};

pub use audit::{Audit, AuditEvent, Snapshot};
pub use state::{Entry, State};

pub const BLOCKED: &str = "blocked";
pub const KEPT: &str = "kept";
pub const FOLLOWED: &str = "followed";
/// Deferred or baselined; hidden unless `--backlog`.
pub const SKIPPED: &str = "skipped";

/// `$XDG_STATE_HOME/bskyban` (only if absolute), else `~/.local/state/bskyban`.
pub fn state_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("XDG_STATE_HOME") {
        let d = PathBuf::from(d);
        if d.is_absolute() {
            return Ok(d.join("bskyban"));
        }
    }
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(Path::new(&h).join(".local/state/bskyban")),
        _ => Err("$HOME is not defined".into()),
    }
}

pub fn default_state_path() -> Result<PathBuf, String> {
    state_dir().map(|d| d.join("state.json"))
}

pub fn default_audit_path() -> Result<PathBuf, String> {
    state_dir().map(|d| d.join("audit.jsonl"))
}
