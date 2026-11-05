//! Command-line surface: clap definitions, environment resolution and dispatch.
//!
//! Syntax is idiomatic clap (see `docs/compat.md`); environment variables, command names,
//! flag names/defaults, exit codes (0 ok, 1 error, 2 usage) and file formats match bskyban.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::app::App;
use crate::bsky::{self, Client, Post};
use crate::commands;
use crate::store::{self, State};
use crate::termtext::{collapse_ws, human_age, sanitize, truncate_width};
use crate::tui::{self, theme};

/// Shortest allowed interval between background checks.
pub const MIN_REFRESH: Duration = Duration::from_secs(10);

const AFTER_HELP: &str = "\
Environment:
  BSKY_HANDLE, BSKY_APP_PASSWORD   required (use an app password)
  BSKY_PDS                         optional, default https://bsky.social
  BSKYBAN_STATE                    optional state file (default ~/.local/state/bskyban/state.json)
  BSKYBAN_AUDIT                    optional audit log (default ~/.local/state/bskyban/audit.jsonl)
  XDG_STATE_HOME                   moves the default state directory (must be absolute)
  BSKYBAN_FANCY                    1 = same as --fancy (highlight newly inserted posts)
  BSKYBAN_THEME                    auto (default), dark, light, ansi, dracula, catppuccin
  BROWSER                          opener for links (default open / xdg-open)
  NO_COLOR                         disable colour";

#[derive(Parser, Debug)]
#[command(
    name = "rsbsky",
    version,
    about = "rsbsky - review and block Bluesky followers (Rust port of bskyban)",
    after_help = AFTER_HELP,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
    /// With no command, `feed` runs: these are its options.
    #[command(flatten)]
    pub feed: FeedArgs,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Your timeline, plus review of repost authors (the default)
    Feed(FeedArgs),
    /// Review new followers one at a time
    Review(ReviewArgs),
    /// Browse your own posts (TUI), or print them
    Posts(PostsArgs),
    /// List followers needing a decision
    List(ListArgs),
    /// Show the audit log (no login needed)
    Log(LogArgs),
    /// Preview colour themes (no login needed)
    Themes,
    /// Mark all current non-followed followers as old
    Baseline,
    /// Show a profile and its recent posts
    Show {
        /// Handle or DID
        who: String,
    },
    /// Block accounts
    Block(ActArgs),
    /// Unblock accounts
    Unblock(ActArgs),
    /// Follow accounts
    Follow(ActArgs),
    /// Unfollow accounts
    Unfollow(ActArgs),
    /// Mark accounts as kept
    Keep(ActArgs),
    /// Publish a post (use - to read the text from stdin)
    Post(PostArgs),
    /// Delete your own posts (at:// URI, bsky.app link or rkey)
    #[command(name = "delete-post")]
    DeletePost(PostRefsArgs),
    /// Like posts (at:// URI or bsky.app link)
    Like(PostRefsArgs),
    /// Remove likes
    Unlike(PostRefsArgs),
}

#[derive(Args, Debug, Default)]
pub struct FeedArgs {
    /// Timeline pages (100 posts each) to scan for repost authors
    #[arg(long, default_value_t = 3)]
    pub pages: u32,
    /// Include authors you skipped earlier
    #[arg(long)]
    pub backlog: bool,
    /// Background check interval (0 = off, otherwise at least 10s)
    #[arg(long, default_value = "30s", value_parser = parse_refresh)]
    pub refresh: Duration,
    /// Highlight newly inserted posts
    #[arg(long)]
    pub fancy: bool,
    /// Colour theme (default BSKYBAN_THEME or auto)
    #[arg(long)]
    pub theme: Option<String>,
    /// Print repost authors instead of starting the TUI
    #[arg(long)]
    pub list: bool,
    /// JSON Lines output (with --list)
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    /// Only the newest N followers
    #[arg(long, default_value_t = 0)]
    pub since: i64,
    /// Include followers you skipped earlier
    #[arg(long)]
    pub backlog: bool,
    #[arg(long)]
    pub theme: Option<String>,
}

#[derive(Args, Debug)]
pub struct PostsArgs {
    #[arg(long, default_value = "30s", value_parser = parse_refresh)]
    pub refresh: Duration,
    #[arg(long)]
    pub fancy: bool,
    #[arg(long)]
    pub theme: Option<String>,
    /// Print instead of starting the TUI
    #[arg(long)]
    pub list: bool,
    #[arg(long)]
    pub json: bool,
    /// How many posts to print
    #[arg(short = 'n', long = "count", default_value_t = 20)]
    pub n: i64,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[arg(long, default_value_t = 0)]
    pub since: i64,
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub backlog: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct LogArgs {
    /// Only events newer than this (48h, 7d)
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub action: Option<String>,
    /// Handle (substring) or DID
    #[arg(long)]
    pub who: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(short = 'n', long = "count", default_value_t = 50)]
    pub n: i64,
}

#[derive(Args, Debug)]
pub struct ActArgs {
    /// Show what would happen without doing it
    #[arg(long)]
    pub dry_run: bool,
    /// Handles or DIDs (use - to read from stdin)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

#[derive(Args, Debug)]
pub struct PostArgs {
    #[arg(long)]
    pub dry_run: bool,
    /// Post text (use - to read it from stdin)
    #[arg(required = true, allow_hyphen_values = true)]
    pub text: Vec<String>,
}

#[derive(Args, Debug)]
pub struct PostRefsArgs {
    #[arg(long)]
    pub dry_run: bool,
    #[arg(required = true)]
    pub posts: Vec<String>,
}

/// `0` turns refresh off; anything else needs a unit (`30s`, `1m`) and at least 10 seconds.
fn parse_refresh(s: &str) -> Result<Duration, String> {
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let d = humantime::parse_duration(s).map_err(|e| e.to_string())?;
    if !d.is_zero() && d < MIN_REFRESH {
        return Err("must be 0 (off) or at least 10s".into());
    }
    Ok(d)
}

/// How a command ends unsuccessfully.
#[derive(Debug)]
pub enum CliError {
    /// Bad invocation: exit 2 and a hint.
    Usage(String),
    /// Runtime failure: `error: ...` and exit 1.
    Failure(String),
    /// Printed verbatim to stderr, exit 1.
    Plain(String),
}

impl From<bsky::Error> for CliError {
    fn from(e: bsky::Error) -> Self {
        CliError::Failure(e.to_string())
    }
}

fn not_implemented(cmd: &str, phase: u8) -> CliError {
    CliError::Failure(format!(
        "{cmd}: not yet implemented in rsbsky (planned: phase {phase})"
    ))
}

fn env(name: &str) -> String {
    std::env::var_os(name)
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether the variable is set to 1, true or yes.
pub fn env_on(name: &str) -> bool {
    matches!(
        env(name).trim().to_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

/// Entry point: returns the process exit status.
pub async fn real_main() -> i32 {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout();
    match run(cli, &mut stdout).await {
        Ok(()) => 0,
        Err(CliError::Plain(m)) => {
            eprintln!("{m}");
            1
        }
        Err(CliError::Failure(m)) => {
            eprintln!("error: {}", sanitize(&m));
            1
        }
        Err(CliError::Usage(m)) => {
            eprintln!(
                "error: {}\n\nFor more information, try '--help'.",
                sanitize(&m)
            );
            2
        }
    }
}

/// Validates `--theme` and `BSKYBAN_THEME` before any credential check. Printed lists
/// ignore the environment default (`posts --json` is headless; `feed --json` alone is not).
fn check_theme(flag: Option<&str>, headless: bool) -> Result<(), CliError> {
    let r = if headless {
        match flag.filter(|f| !f.is_empty()) {
            Some(f) => theme::by_name(f).map(|_| ()),
            None => Ok(()),
        }
    } else {
        theme::resolve(flag, &env("BSKYBAN_THEME")).map(|_| ())
    };
    r.map_err(|e| CliError::Usage(e.to_string()))
}

pub async fn run(cli: Cli, out: &mut dyn Write) -> Result<(), CliError> {
    let default_feed;
    let cmd = match cli.command {
        Some(c) => c,
        None => {
            default_feed = cli.feed;
            Cmd::Feed(default_feed)
        }
    };

    // Commands that never log in.
    match &cmd {
        Cmd::Log(a) => return commands::run_log(a, out),
        Cmd::Themes => {
            let ansi = std::io::stdout().is_terminal();
            return theme::print_themes(out, ansi).map_err(|e| CliError::Failure(e.to_string()));
        }
        _ => {}
    }

    // TUI-style commands validate their options before credentials are looked at.
    match &cmd {
        Cmd::Feed(a) => check_theme(a.theme.as_deref(), a.list)?,
        Cmd::Posts(a) => check_theme(a.theme.as_deref(), a.list || a.json)?,
        Cmd::Review(a) => check_theme(a.theme.as_deref(), false)?,
        _ => {}
    }

    let (handle, password) = (env("BSKY_HANDLE"), env("BSKY_APP_PASSWORD"));
    if handle.is_empty() || password.is_empty() {
        return Err(CliError::Plain(
            "BSKY_HANDLE and BSKY_APP_PASSWORD must be set".into(),
        ));
    }
    let pds = match env("BSKY_PDS") {
        p if p.is_empty() => "https://bsky.social".to_string(),
        p => p,
    };
    let state_path = match env("BSKYBAN_STATE") {
        p if p.is_empty() => store::default_state_path().map_err(CliError::Failure)?,
        p => PathBuf::from(p),
    };
    // The audit path is resolved now (and can fail) exactly as in bskyban, even though
    // audit writing arrives in Phase 2.
    let audit_path = match env("BSKYBAN_AUDIT") {
        p if p.is_empty() => store::default_audit_path().map_err(CliError::Failure)?,
        p => PathBuf::from(p),
    };
    let state = State::load(state_path).map_err(|e| CliError::Failure(e.to_string()))?;
    let mut client = Client::new(&pds);
    client
        .login(&handle, &password)
        .await
        .map_err(|e| CliError::Failure(format!("login: {e}")))?;
    let app = App {
        client,
        state,
        audit: store::Audit::new(audit_path),
    };

    match &cmd {
        Cmd::Feed(a) if a.list => {
            let tw = terminal_width();
            feed_list(&app, a.pages as usize, a.backlog, a.json, out, tw).await?;
            Ok(())
        }
        Cmd::Feed(a) => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(CliError::Failure(
                    "the feed view needs a terminal (use --list for plain output)".into(),
                ));
            }
            let name = theme::resolve(a.theme.as_deref(), &env("BSKYBAN_THEME"))
                .map_err(|e| CliError::Usage(e.to_string()))?;
            let opts = tui::FeedOptions {
                theme: name,
                refresh: a.refresh,
                fancy: a.fancy || env_on("BSKYBAN_FANCY"),
            };
            tui::run_feed(app, opts)
                .await
                .map_err(|e| CliError::Failure(e.to_string()))
        }
        Cmd::Posts(_) => Err(not_implemented("posts", 6)),
        Cmd::Review(_) => Err(not_implemented("review", 5)),
        Cmd::Post(_) | Cmd::DeletePost(_) | Cmd::Like(_) | Cmd::Unlike(_) => {
            Err(not_implemented("post commands", 6))
        }
        headless => {
            let stdin = std::io::stdin();
            let mut lock = stdin.lock();
            let mut ctx = commands::Ctx {
                app,
                stdin: &mut lock,
                out,
                stdout_is_tty: std::io::stdout().is_terminal(),
            };
            match headless {
                Cmd::List(a) => ctx.cmd_list(a.since, a.all, a.backlog, a.json).await,
                Cmd::Baseline => ctx.cmd_baseline().await,
                Cmd::Show { who } => ctx.cmd_show(who).await,
                Cmd::Block(a) => ctx.cmd_act("block", &a.ids, a.dry_run).await,
                Cmd::Unblock(a) => ctx.cmd_act("unblock", &a.ids, a.dry_run).await,
                Cmd::Follow(a) => ctx.cmd_act("follow", &a.ids, a.dry_run).await,
                Cmd::Unfollow(a) => ctx.cmd_act("unfollow", &a.ids, a.dry_run).await,
                Cmd::Keep(a) => ctx.cmd_act("keep", &a.ids, a.dry_run).await,
                _ => unreachable!("handled above"),
            }
        }
    }
}

/// Width of stdout if it is a terminal, else `None` (piped: never truncate).
fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
        .filter(|w| *w > 0)
}

#[derive(Serialize)]
struct JsonLine<'a> {
    did: &'a str,
    #[serde(rename = "displayName")]
    display_name: &'a str,
    handle: &'a str,
    #[serde(rename = "repostedBy")]
    reposted_by: &'a [String],
    text: &'a str,
}

/// Prints the repost authors the Reposts tab would offer (`feed --list`).
pub async fn feed_list(
    app: &App,
    pages: usize,
    backlog: bool,
    as_json: bool,
    out: &mut dyn Write,
    term_width: Option<usize>,
) -> Result<(), bsky::Error> {
    let (list, rc) = app.feed_candidates(pages, backlog).await?;
    for p in list {
        let r = &rc[&p.did];
        if as_json {
            let line = JsonLine {
                did: &p.did,
                display_name: &p.display_name,
                handle: &p.handle,
                reposted_by: &r.by,
                text: &r.post.text,
            };
            if let Ok(s) = serde_json::to_string(&line) {
                let _ = writeln!(out, "{}", store::audit::go_json(s));
            }
            continue;
        }
        let mut line = format!(
            "{:<32} via {:<40} {}",
            p.handle,
            format!("@{}", r.by.join(", @")),
            format_post(&r.post, Utc::now())
        );
        if let Some(tw) = term_width {
            line = truncate_width(&line, tw.saturating_sub(1)); // fit exactly; piped output is never cut
        }
        let _ = writeln!(out, "{line}");
    }
    Ok(())
}

/// Renders a post as a single line: `<age> [tag] text`.
pub fn format_post(p: &Post, now: DateTime<Utc>) -> String {
    let tag = if p.repost {
        "[repost] "
    } else if p.reply {
        "[reply] "
    } else {
        ""
    };
    let age = DateTime::parse_from_rfc3339(&p.created_at)
        .map(|t| human_age(now.signed_duration_since(t)))
        .unwrap_or_else(|_| "?".into());
    let mut text = collapse_ws(&sanitize(&p.text));
    if !p.attach.is_empty() {
        text = format!("{text} {}", sanitize(&p.attach.join(" ")))
            .trim()
            .to_string();
    }
    if let Some(c) = &p.link {
        // full URL here: CLI output is copied and piped
        let card = if !c.title.is_empty() && !p.text.contains(&c.uri) {
            format!("↗ {} {}", c.title, c.uri)
        } else if !c.title.is_empty() {
            format!("↗ {}", c.title)
        } else {
            format!("↗ {}", c.uri)
        };
        text = format!("{text} {}", sanitize(&card)).trim().to_string();
    }
    if text.is_empty() {
        text = "(no text)".into();
    }
    format!("{age:<5} {tag}{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsky::LinkCard;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn refresh_bounds() {
        assert_eq!(parse_refresh("0").unwrap(), Duration::ZERO);
        assert_eq!(parse_refresh("0s").unwrap(), Duration::ZERO);
        assert_eq!(parse_refresh("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_refresh("1m30s").unwrap(), Duration::from_secs(90));
        assert!(parse_refresh("9s").is_err());
        assert!(parse_refresh("5").is_err());
        assert!(parse_refresh("fast").is_err());
    }

    #[test]
    fn default_command_is_feed() {
        let c = Cli::try_parse_from(["rsbsky", "--list", "--json", "--pages", "2"]).unwrap();
        assert!(c.command.is_none());
        assert!(c.feed.list && c.feed.json && c.feed.pages == 2);
        assert_eq!(c.feed.refresh, Duration::from_secs(30));
        assert!(Cli::try_parse_from(["rsbsky", "feed", "--refresh", "3s"]).is_err());
        assert!(Cli::try_parse_from(["rsbsky", "bogus"]).is_err());
    }

    #[test]
    fn post_lines() {
        let now = Utc::now();
        let mut p = Post {
            text: " hello\n\x1b[31mworld ".into(),
            created_at: (now - chrono::Duration::minutes(7)).to_rfc3339(),
            reply: true,
            attach: vec!["▣ cat".into()],
            link: Some(LinkCard {
                uri: "https://e.com".into(),
                title: "Title".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            format_post(&p, now),
            "7m    [reply] hello world ▣ cat ↗ Title https://e.com"
        );
        p = Post::default();
        assert_eq!(format_post(&p, now), "?     (no text)");
    }

    #[test]
    fn json_escapes_like_go() {
        assert_eq!(
            store::audit::go_json("[\"<&>\"]".into()),
            "[\"\\u003c\\u0026\\u003e\"]"
        );
    }
}
