//! The async event loop around [`FeedApp`]: terminal lifecycle, input, timers, and the
//! background requests whose results come back as messages.
//!
//! Requests run as spawned tasks, so slow HTTP never blocks input or redraws and quitting
//! is immediate: shutdown aborts every outstanding task.

use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at};
use tracing::Instrument;

use super::actions::run_act;
use super::feed::RECENT_POSTS;
use super::feed::{ANIM_TICK, CHECK_PAGES, CheckOutcome, Effect, FeedApp, Msg, Now, PAGE_SIZE};
use super::links::{LinkMode, LinkPass};
use super::picker::open_url;
use super::render::render;
use crate::bsky::Client;
use crate::store::Audit;

/// Puts the terminal in raw mode on the alternate screen and restores it on drop, whether
/// the program ends normally, with an error, or by unwinding from a panic.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen, crossterm::cursor::Hide) {
            let _ = disable_raw_mode(); // partial initialization: undo what succeeded
            return Err(e);
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Idempotent: closes any open hyperlink, resets styling, shows the cursor, leaves the
/// alternate screen and raw mode.
pub fn restore_terminal() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b]8;;\x1b\\\x1b[0m");
    let _ = execute!(out, crossterm::cursor::Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Restores the terminal before the default panic message prints, so it is readable.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Reads up to [`CHECK_PAGES`] pages of the timeline and returns the posts that are not in
/// `known`, stopping at the first page that overlaps it or when the cursor ends. Reports a
/// gap when the limit is reached without any overlap.
pub async fn run_check(client: &Client, known: &HashSet<String>) -> Result<CheckOutcome, String> {
    let mut fresh = Vec::new();
    let mut got = HashSet::new();
    let mut cursor = String::new();
    for _ in 0..CHECK_PAGES {
        let (items, next) = client
            .get_timeline(&cursor, PAGE_SIZE)
            .await
            .map_err(|e| e.to_string())?;
        let mut overlap = false;
        for it in items {
            let key = it.key();
            if known.contains(&key) {
                overlap = true;
            } else if got.insert(key) {
                fresh.push(it);
            }
        }
        if overlap || next.is_empty() {
            return Ok(CheckOutcome {
                items: fresh,
                gap: false,
            });
        }
        cursor = next;
    }
    Ok(CheckOutcome {
        items: fresh,
        gap: true,
    })
}

async fn tick_or_pending(i: &mut Option<Interval>) {
    match i {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// Runs the feed UI until the user quits.
pub async fn run(mut app: FeedApp, client: Client, audit: Audit) -> io::Result<()> {
    let client = Arc::new(client);
    install_panic_hook();
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    // No terminal.clear(): it asks the terminal for the cursor position (ESC[6n) and fails
    // when the terminal does not answer. The alternate screen is already blank.
    let size = terminal.size()?;
    app.on_msg(Msg::Resize(size.width, size.height), Now::current());

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut events = EventStream::new();
    let mut refresh = (!app.refresh.is_zero()).then(|| {
        let mut i = interval_at(Instant::now() + app.refresh, app.refresh);
        i.set_missed_tick_behavior(MissedTickBehavior::Delay);
        i
    });
    let mut spin = tokio::time::interval(Duration::from_millis(100));
    let mut anim_at: Option<Instant> = None;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let mut links = LinkPass::new(LinkMode::detect());

    let mut pending = app.start();
    let mut quit = false;
    loop {
        // perform effects
        for effect in std::mem::take(&mut pending) {
            match effect {
                Effect::Quit => quit = true,
                Effect::ScheduleAnim => anim_at = Some(Instant::now() + ANIM_TICK),
                Effect::Audit(ev) => {
                    if let Err(e) = audit.log(*ev) {
                        pending.extend(app.on_msg(Msg::AuditFailed(e.to_string()), Now::current()));
                    }
                }
                Effect::FetchProfile { did } => {
                    let (c1, tx1, d1) = (client.clone(), tx.clone(), did.clone());
                    let span = tracing::info_span!("fetch profile", logfire.msg = "fetch profile");
                    tasks.spawn(
                        async move {
                            let result = c1
                                .get_profile(&d1)
                                .await
                                .map(Box::new)
                                .map_err(|e| e.to_string());
                            let _ = tx1.send(Msg::Profile { did: d1, result });
                        }
                        .instrument(span),
                    );
                    let (c, tx) = (client.clone(), tx.clone());
                    let span = tracing::info_span!(
                        "fetch recent posts",
                        logfire.msg = "fetch recent posts"
                    );
                    tasks.spawn(
                        async move {
                            let result = c
                                .get_recent_posts(&did, RECENT_POSTS)
                                .await
                                .map_err(|e| e.to_string());
                            let _ = tx.send(Msg::Posts { did, result });
                        }
                        .instrument(span),
                    );
                }
                Effect::Open {
                    urls,
                    status,
                    err_prefix,
                } => {
                    // launching is non-blocking; stop at the first failure like Go does
                    let result = urls
                        .iter()
                        .try_for_each(|u| open_url(u))
                        .map_err(|e| e.to_string());
                    pending.extend(app.on_msg(
                        Msg::OpenDone {
                            result,
                            status,
                            err_prefix,
                        },
                        Now::current(),
                    ));
                }
                Effect::Act(req) => {
                    // Not aborted on quit while busy: the loop keeps running until the result
                    // arrives, so the action is always saved and audited.
                    let (c, tx) = (client.clone(), tx.clone());
                    let kind = req.kind();
                    let span = tracing::info_span!(
                        "moderation action",
                        logfire.msg = %format_args!("moderation action: {kind}"),
                        kind,
                        outcome = tracing::field::Empty,
                        lifted = tracing::field::Empty,
                    );
                    tasks.spawn(
                        async move {
                            let msg = run_act(&c, *req).await;
                            let span = tracing::Span::current();
                            span.record("outcome", if msg.err.is_none() { "ok" } else { "error" });
                            span.record("lifted", msg.lifted);
                            let _ = tx.send(Msg::Act(Box::new(msg)));
                        }
                        .instrument(span),
                    );
                }
                Effect::LoadFollows { req, actor, cursor } => {
                    let (c, tx) = (client.clone(), tx.clone());
                    let span = tracing::info_span!(
                        "load follows page",
                        logfire.msg = "load follows page",
                        first = cursor.is_empty()
                    );
                    tasks.spawn(
                        async move {
                            let result = c
                                .get_follows_page(&actor, &cursor)
                                .await
                                .map_err(|e| e.to_string());
                            let _ = tx.send(Msg::FollowsPage { req, result });
                        }
                        .instrument(span),
                    );
                }
                Effect::LoadPage { id, cursor } => {
                    let (c, tx) = (client.clone(), tx.clone());
                    let span = tracing::info_span!(
                        "load timeline page",
                        logfire.msg = "load timeline page",
                        first = cursor.is_empty(),
                        items = tracing::field::Empty,
                    );
                    tasks.spawn(
                        async move {
                            let result = c
                                .get_timeline(&cursor, PAGE_SIZE)
                                .await
                                .map_err(|e| e.to_string());
                            if let Ok((items, _)) = &result {
                                tracing::Span::current().record("items", items.len());
                            }
                            let _ = tx.send(Msg::Page { id, result });
                        }
                        .instrument(span),
                    );
                }
                Effect::Check { id, manual, known } => {
                    let (c, tx) = (client.clone(), tx.clone());
                    let span = tracing::info_span!(
                        "refresh check",
                        logfire.msg = "refresh check",
                        manual,
                        outcome = tracing::field::Empty,
                        new_posts = tracing::field::Empty,
                        gap = tracing::field::Empty,
                    );
                    tasks.spawn(
                        async move {
                            let result = run_check(&c, &known).await;
                            let span = tracing::Span::current();
                            match &result {
                                Ok(o) => {
                                    span.record("outcome", "ok");
                                    span.record("new_posts", o.items.len());
                                    span.record("gap", o.gap);
                                }
                                Err(_) => {
                                    span.record("outcome", "error");
                                }
                            }
                            let _ = tx.send(Msg::Check { id, manual, result });
                        }
                        .instrument(span),
                    );
                }
            }
        }
        if quit {
            break;
        }

        // draw, then lay hyperlinks over the flushed frame
        let now = Now::current();
        let mut regions = Vec::new();
        let frame = terminal.draw(|f| regions = render(&app, f.buffer_mut(), now))?;
        let bytes = links.bytes(frame.buffer, regions);
        if !bytes.is_empty() {
            let w = terminal.backend_mut();
            w.write_all(&bytes)?;
            w.flush()?;
        }

        let anim_sleep = async {
            match anim_at {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending().await,
            }
        };
        let msgs: Vec<Msg> = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                    pending.extend(app.on_key(k, Now::current()));
                    Vec::new()
                }
                Some(Ok(Event::Resize(w, h))) => vec![Msg::Resize(w, h)],
                Some(Ok(_)) => Vec::new(),
                Some(Err(e)) => return Err(e),
                None => break, // input closed
            },
            Some(m) = rx.recv() => vec![m],
            _ = tick_or_pending(&mut refresh) => vec![Msg::RefreshTick],
            _ = spin.tick(), if app.needs_spin() => vec![Msg::SpinTick],
            _ = anim_sleep => { anim_at = None; vec![Msg::AnimTick] },
            _ = sigterm.recv() => { quit = true; Vec::new() },
            _ = sighup.recv() => { quit = true; Vec::new() },
        };
        for m in msgs {
            pending.extend(app.on_msg(m, Now::current()));
        }
        if quit {
            break;
        }
    }

    // Outstanding requests must not keep the process alive or the terminal busy.
    tasks.abort_all();
    Ok(())
}
