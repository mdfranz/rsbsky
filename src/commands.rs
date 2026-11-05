//! Headless subcommands: `log`, `list`, `baseline`, `show` and the decision commands
//! (`block`, `unblock`, `follow`, `unfollow`, `keep`). Port of Go `internal/cli`.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Local, Utc};
use serde::Serialize;

use crate::app::App;
use crate::bsky::Profile;
use crate::cli::{CliError, LogArgs, format_post};
use crate::store::{self, AuditEvent, Snapshot};
use crate::termtext::{human_age, sanitize};

/// What the subcommands read from and write to.
pub struct Ctx<'a> {
    pub app: App,
    pub stdin: &'a mut dyn BufRead,
    pub out: &'a mut dyn Write,
    pub stdout_is_tty: bool,
}

impl Ctx<'_> {
    /// Writes an audit event; a failing log is reported but never hides the action's result.
    fn record(&self, mut e: AuditEvent) {
        if e.source.is_empty() {
            e.source = "cli".into();
        }
        if let Err(err) = self.app.audit.log(e) {
            eprintln!("warning: audit log: {err}");
        }
    }

    fn linkify_if_tty(&self, s: &str) -> String {
        let s = sanitize(s);
        if self.stdout_is_tty { linkify(&s) } else { s }
    }
}

/// Wraps URLs and bare domains in OSC 8 hyperlinks so supporting terminals make them clickable.
fn linkify(s: &str) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for l in crate::termtext::find_links(s) {
        out.push_str(&s[pos..l.start]);
        out.push_str(&format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            l.target, l.text
        ));
        out.push_str(&s[l.start + l.text.len()..l.end]);
        pos = l.end;
    }
    out.push_str(&s[pos..]);
    out
}

fn normalize_actor(s: &str) -> String {
    s.trim().trim_start_matches('@').to_string()
}

// ------------------------------------------------------------------------------------ log

/// Go's `parseSince`: Go durations plus a `d` suffix for days (`7d`; `2d12h` is not supported).
fn parse_since(s: &str) -> Result<Duration, String> {
    if let Some(days) = s.strip_suffix('d') {
        let n: f64 = days.parse().map_err(|_| format!("bad --since {s:?}"))?;
        return Ok(Duration::milliseconds((n * 24.0 * 3_600_000.0) as i64));
    }
    humantime::parse_duration(s)
        .ok()
        .and_then(|d| Duration::from_std(d).ok())
        .ok_or_else(|| format!("bad --since {s:?} (try 48h or 7d)"))
}

/// `rsbsky log`: shows the audit log. Needs no login.
pub fn run_log(args: &LogArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let path = match std::env::var("BSKYBAN_AUDIT")
        .ok()
        .filter(|p| !p.is_empty())
    {
        Some(p) => PathBuf::from(p),
        None => store::default_audit_path().map_err(CliError::Failure)?,
    };
    let events = store::read_audit(&path).map_err(CliError::Failure)?;
    let cutoff = match args.since.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(Utc::now() - parse_since(s).map_err(CliError::Usage)?),
        None => None,
    };
    let who = args.who.as_deref().unwrap_or("").trim_start_matches('@');
    let action = args.action.as_deref().unwrap_or("");
    let mut shown: Vec<&AuditEvent> = events
        .iter()
        .filter(|e| {
            if let (Some(cut), Ok(t)) = (cutoff, DateTime::parse_from_rfc3339(&e.ts))
                && t.with_timezone(&Utc) < cut
            {
                return false;
            }
            if !action.is_empty() && e.action != action {
                return false;
            }
            who.is_empty() || e.handle.contains(who) || e.did == who
        })
        .collect();
    if args.n > 0 && shown.len() > args.n as usize {
        shown = shown.split_off(shown.len() - args.n as usize);
    }
    for e in shown {
        if args.json {
            if let Ok(s) = serde_json::to_string(e) {
                let _ = writeln!(out, "{}", store::audit::go_json(s));
            }
            continue;
        }
        let ts = match DateTime::parse_from_rfc3339(&e.ts) {
            Ok(t) => t
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            Err(_) => e.ts.clone(),
        };
        let mut line = format!(
            "{ts}  {:<8} {:<34} {}",
            e.action,
            format!("@{}", e.handle),
            e.source
        );
        if !e.via.is_empty() {
            line.push_str(&format!("  via @{}", e.via));
        }
        if e.dry_run {
            line.push_str("  [dry-run]");
        }
        if !e.ok {
            line.push_str(&format!("  FAILED: {}", e.error));
        }
        if !e.detail.is_empty() {
            line.push_str(&format!("  ({})", e.detail));
        }
        let _ = writeln!(out, "{}", sanitize(&line));
    }
    Ok(())
}

// ----------------------------------------------------------------------------------- list

#[derive(Serialize)]
struct ListLine<'a> {
    #[serde(rename = "createdAt")]
    created_at: &'a str,
    description: &'a str,
    did: &'a str,
    #[serde(rename = "displayName")]
    display_name: &'a str,
    followers: i64,
    following: i64,
    handle: &'a str,
    #[serde(rename = "hasAvatar")]
    has_avatar: bool,
    posts: i64,
    status: &'a str,
}

impl Ctx<'_> {
    pub async fn cmd_list(
        &mut self,
        since: i64,
        all: bool,
        backlog: bool,
        as_json: bool,
    ) -> Result<(), CliError> {
        let list = self.app.followers(since, all, backlog).await?;
        let dids: Vec<String> = list.iter().map(|f| f.did.clone()).collect();
        let profiles = self.app.client.get_profiles(&dids).await?;
        let by_did: std::collections::HashMap<&str, &Profile> =
            profiles.iter().map(|p| (p.did.as_str(), p)).collect();
        for f in &list {
            let p = by_did.get(f.did.as_str()).copied().unwrap_or(f);
            let status = match self.app.state.decision(&p.did) {
                "" => "new",
                d => d,
            };
            if as_json {
                let line = ListLine {
                    created_at: &p.created_at,
                    description: &p.description,
                    did: &p.did,
                    display_name: &p.display_name,
                    followers: p.followers_count,
                    following: p.follows_count,
                    handle: &p.handle,
                    has_avatar: !p.avatar.is_empty(),
                    posts: p.posts_count,
                    status,
                };
                if let Ok(s) = serde_json::to_string(&line) {
                    let _ = writeln!(self.out, "{}", store::audit::go_json(s));
                }
                continue;
            }
            let _ = writeln!(
                self.out,
                "{:<6} {:<32} posts={:<5} followers={:<6} following={:<6} age={:<8} {}",
                status,
                p.handle,
                p.posts_count,
                p.followers_count,
                p.follows_count,
                human_age(p.age()),
                p.did
            );
        }
        Ok(())
    }

    /// Marks every follower without a decision as skipped, so only followers who arrive
    /// afterwards show up by default.
    pub async fn cmd_baseline(&mut self) -> Result<(), CliError> {
        let list = self.app.followers(0, false, false).await?;
        for f in &list {
            self.app
                .state
                .entries
                .insert(f.did.clone(), store::Entry::now(&f.handle, store::SKIPPED));
        }
        self.app
            .state
            .save()
            .map_err(|e| CliError::Failure(e.to_string()))?;
        self.record(AuditEvent {
            action: "baseline".into(),
            ok: true,
            detail: format!("{} followers marked as old", list.len()),
            ..Default::default()
        });
        let _ = writeln!(
            self.out,
            "baselined {} existing followers; only new ones will show by default (--backlog to revisit)",
            list.len()
        );
        Ok(())
    }

    pub async fn cmd_show(&mut self, who: &str) -> Result<(), CliError> {
        let p = self.app.client.get_profile(&normalize_actor(who)).await?;
        let mut status = self.app.state.decision(&p.did).to_string();
        if !p.viewer.blocking.is_empty() {
            status = "blocked".into();
        }
        if status.is_empty() {
            status = "new".into();
        }
        let bio = self.linkify_if_tty(&p.description);
        let _ = write!(
            self.out,
            "{} (@{})\nDID:       {}\nStatus:    {}\nCreated:   {} ({})\nPosts:     {}\nFollowers: {}\nFollowing: {}\nAvatar:    {}\nURL:       https://bsky.app/profile/{}\n\n{}\n",
            p.display_name,
            p.handle,
            p.did,
            status,
            p.created_at,
            human_age(p.age()),
            p.posts_count,
            p.followers_count,
            p.follows_count,
            !p.avatar.is_empty(),
            p.handle,
            bio
        );
        let posts = match self.app.client.get_recent_posts(&p.did, 5).await {
            Ok(posts) => posts,
            Err(e) => {
                eprintln!("recent posts: {e}");
                return Ok(());
            }
        };
        let _ = write!(self.out, "\nRecent posts:\n");
        if posts.is_empty() {
            let _ = writeln!(self.out, "  (none)");
        }
        for post in &posts {
            let line = self.linkify_if_tty(&format_post(post, Utc::now()));
            let _ = writeln!(self.out, "  {line}");
        }
        Ok(())
    }

    /// Expands `-` into the first whitespace-separated field of each stdin line.
    fn identifiers(&mut self, args: &[String]) -> Result<Vec<String>, CliError> {
        let mut ids = Vec::new();
        for arg in args {
            if arg != "-" {
                ids.push(normalize_actor(arg));
                continue;
            }
            let mut line = String::new();
            loop {
                line.clear();
                match self.stdin.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Some(f) = line.split_whitespace().next() {
                            ids.push(normalize_actor(f));
                        }
                    }
                    Err(e) => return Err(CliError::Failure(e.to_string())),
                }
            }
        }
        Ok(ids)
    }

    /// `block`, `unblock`, `follow`, `unfollow` and `keep` for each identifier.
    pub async fn cmd_act(&mut self, cmd: &str, args: &[String], dry: bool) -> Result<(), CliError> {
        let ids = self.identifiers(args)?;
        if ids.is_empty() {
            return Err(CliError::Usage(format!(
                "{cmd} needs at least one handle or DID"
            )));
        }
        let mut failed = 0;
        for id in &ids {
            if let Err(e) = self.act(cmd, id, dry).await {
                eprintln!("{id}: {e}");
                failed += 1;
            }
        }
        if failed > 0 {
            return Err(CliError::Failure(format!(
                "{failed} of {} failed",
                ids.len()
            )));
        }
        Ok(())
    }

    async fn act(&mut self, cmd: &str, id: &str, dry: bool) -> Result<(), String> {
        let p = match self.app.client.get_profile(id).await {
            Ok(p) => p,
            Err(e) => {
                let e = e.to_string();
                self.record(AuditEvent {
                    action: cmd.into(),
                    handle: id.into(),
                    dry_run: dry,
                    error: e.clone(),
                    ..Default::default()
                });
                return Err(e);
            }
        };
        let mut ev = AuditEvent {
            action: cmd.into(),
            did: p.did.clone(),
            handle: p.handle.clone(),
            dry_run: dry,
            prev: self.app.state.decision(&p.did).to_string(),
            snapshot: Some(Snapshot::of(&p, true)),
            ..Default::default()
        };
        let result = self.act_inner(cmd, &p, dry, &mut ev).await;
        ev.ok = result.is_ok();
        if let Err(e) = &result {
            ev.error = e.clone();
        }
        self.record(ev);
        result
    }

    async fn act_inner(
        &mut self,
        cmd: &str,
        p: &Profile,
        dry: bool,
        ev: &mut AuditEvent,
    ) -> Result<(), String> {
        let prefix = if dry { "[dry-run] would " } else { "" };
        let done_prefix = prefix.replace("would ", "");
        let (client, did, handle) = (&self.app.client, p.did.as_str(), p.handle.as_str());
        let io = |e: std::io::Error| e.to_string();
        match cmd {
            "block" => {
                if !p.viewer.blocking.is_empty() {
                    let _ = writeln!(self.out, "{done_prefix}@{handle} is already blocked");
                    ev.detail = "already blocked".into();
                    ev.record = p.viewer.blocking.clone();
                    if dry {
                        return Ok(());
                    }
                    return self.app.state.set(did, handle, store::BLOCKED).map_err(io);
                }
                let _ = writeln!(self.out, "{prefix}block @{handle} ({did})");
                if dry {
                    return Ok(());
                }
                ev.record = client.block(did).await.map_err(|e| e.to_string())?;
                self.app.state.set(did, handle, store::BLOCKED).map_err(io)
            }
            "keep" => {
                let _ = writeln!(self.out, "{prefix}keep @{handle}");
                if dry {
                    return Ok(());
                }
                self.app.state.set(did, handle, store::KEPT).map_err(io)
            }
            "follow" => {
                if !p.viewer.following.is_empty() {
                    let _ = writeln!(self.out, "{done_prefix}@{handle}: you already follow them");
                    ev.detail = "already following".into();
                    ev.record = p.viewer.following.clone();
                    return Ok(());
                }
                let _ = writeln!(self.out, "{prefix}follow @{handle} ({did})");
                if dry {
                    return Ok(());
                }
                ev.record = client.follow(did).await.map_err(|e| e.to_string())?;
                self.app.state.set(did, handle, store::FOLLOWED).map_err(io)
            }
            "unfollow" => {
                let _ = writeln!(self.out, "{prefix}unfollow @{handle} ({did})");
                ev.record = p.viewer.following.clone();
                if dry {
                    return Ok(());
                }
                if p.viewer.following.is_empty() {
                    ev.detail = "not following".into();
                    return Ok(());
                }
                client
                    .unfollow(&p.viewer.following)
                    .await
                    .map_err(|e| e.to_string())?;
                if self.app.state.decision(did) == store::FOLLOWED {
                    return self.app.state.delete(did).map_err(io);
                }
                Ok(())
            }
            "unblock" => {
                let _ = writeln!(self.out, "{prefix}unblock @{handle} ({did})");
                ev.record = p.viewer.blocking.clone();
                if dry {
                    return Ok(());
                }
                if !p.viewer.blocking.is_empty() {
                    client
                        .unblock(&p.viewer.blocking)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                self.app.state.delete(did).map_err(io)
            }
            _ => Ok(()),
        }
    }
}
