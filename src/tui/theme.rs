//! Theme names and selection. Colours arrive with the TUI in Phase 2; names are
//! validated now so `--theme` / `BSKYBAN_THEME` behave identically from day one.

use thiserror::Error;

/// Theme names in preview/help order.
pub const NAMES: [&str; 6] = ["auto", "dark", "light", "ansi", "dracula", "catppuccin"];

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown theme {0:?} (choose from: {list})", list = NAMES.join(", "))]
pub struct UnknownTheme(pub String);

/// Finds a theme by name, ignoring case; returns its canonical name.
pub fn by_name(name: &str) -> Result<&'static str, UnknownTheme> {
    let lower = name.to_lowercase();
    NAMES
        .iter()
        .copied()
        .find(|n| *n == lower)
        .ok_or_else(|| UnknownTheme(name.to_string()))
}

/// Picks the theme from the `--theme` flag, else the environment, else `auto`.
pub fn resolve(flag: Option<&str>, env: &str) -> Result<&'static str, UnknownTheme> {
    match flag.filter(|f| !f.is_empty()) {
        Some(f) => by_name(f),
        None if !env.is_empty() => by_name(env),
        None => by_name("auto"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        assert_eq!(resolve(None, "").unwrap(), "auto");
        assert_eq!(resolve(None, "Dark").unwrap(), "dark");
        assert_eq!(resolve(Some("light"), "dark").unwrap(), "light");
        assert_eq!(resolve(Some(""), "dracula").unwrap(), "dracula");
        assert!(resolve(None, "nope").is_err());
        assert_eq!(
            by_name("x").unwrap_err().to_string(),
            "unknown theme \"x\" (choose from: auto, dark, light, ansi, dracula, catppuccin)"
        );
    }
}
