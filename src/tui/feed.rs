//! The Following feed view as a pure state machine.
//!
//! Input (keys, timer ticks, request results) goes in through [`FeedApp::on_key`] and
//! [`FeedApp::on_msg`]; side effects come out as [`Effect`]s that the runtime performs.
//! Nothing here touches the terminal, the network, the clock or the filesystem, so refresh
//! ordering, deduplication, gap reporting and error auditing are unit-testable.
//!
//! Ported from the Following tab of Go `internal/tui/ui.go`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::theme::Theme;
use crate::bsky::FeedItem;
use crate::store::AuditEvent;
use crate::termtext::{clip, collapse_ws};

/// Timeline items per page.
pub const PAGE_SIZE: usize = 50;
/// How many pages one background check reads while looking for the overlap with what is shown.
pub const CHECK_PAGES: usize = 4;
/// How long a newly inserted post stays highlighted in fancy mode.
pub const FRESH_FOR: Duration = Duration::from_secs(2);
/// Redraw interval while a highlight fades.
pub const ANIM_TICK: Duration = Duration::from_millis(80);
/// Lines of post text shown in the list.
pub const PREVIEW_LINES: usize = 3;
/// Lines of a quoted post shown in the list.
pub const QUOTE_LINES: usize = 2;
/// Items closer than this to the end of the list trigger loading the next page.
const LOAD_MORE_WITHIN: usize = 8;

/// A reading of the monotonic and wall clocks, injected so tests are deterministic.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub mono: Instant,
    pub wall: DateTime<Utc>,
}

impl Now {
    pub fn current() -> Self {
        Now {
            mono: Instant::now(),
            wall: Utc::now(),
        }
    }

    pub fn plus(self, d: Duration) -> Self {
        Now {
            mono: self.mono + d,
            wall: self.wall + chrono::Duration::from_std(d).unwrap(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutcome {
    /// Newest first; none of them was known when the check started.
    pub items: Vec<FeedItem>,
    /// More new posts arrived than one check looks at.
    pub gap: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// A timeline page requested with [`Effect::LoadPage`].
    Page {
        id: u64,
        result: Result<(Vec<FeedItem>, String), String>,
    },
    /// A check requested with [`Effect::Check`].
    Check {
        id: u64,
        manual: bool,
        result: Result<CheckOutcome, String>,
    },
    /// The refresh timer fired.
    RefreshTick,
    /// The fancy-mode animation timer fired.
    AnimTick,
    /// The spinner timer fired.
    SpinTick,
    Resize(u16, u16),
    /// Writing the audit log failed.
    AuditFailed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    LoadPage {
        id: u64,
        cursor: String,
    },
    Check {
        id: u64,
        manual: bool,
        known: HashSet<String>,
    },
    /// Send [`Msg::AnimTick`] after [`ANIM_TICK`].
    ScheduleAnim,
    Audit(Box<AuditEvent>),
    Quit,
}

/// Cached wrapped text of one list item at one width.
#[derive(Clone, Default)]
pub(super) struct Clipped {
    pub text: Vec<String>,
    pub quote: Vec<String>,
}

pub struct FeedApp {
    pub theme: Theme,
    pub fancy: bool,
    /// Background check interval; zero turns checking off.
    pub refresh: Duration,
    pub me_did: String,
    pub me_handle: String,
    /// Recorded decision per DID, for the blocked/followed/kept tags.
    pub decisions: HashMap<String, String>,

    pub items: Vec<FeedItem>,
    pub cur: usize,
    pub list_top: usize,
    pub tl_cursor: String,
    pub follow_loading: bool,
    pub more_busy: bool,
    /// New posts found by a check but not yet inserted (newest first).
    pub new_posts: Vec<FeedItem>,
    /// ...and there were more than one check could fetch.
    pub new_gap: bool,
    pub checking: bool,
    /// `R` was pressed during a check: insert its result when it lands.
    pub apply_when_done: bool,
    /// The last background check failed.
    pub refresh_err: Option<String>,
    pub fresh: HashMap<String, Instant>,
    pub animating: bool,

    // States that block automatic insertion once later phases add them (detail view,
    // composer, busy actions). Always false in Phase 2, but already consulted.
    pub in_post: bool,
    pub composing: bool,
    pub busy: bool,

    pub status: String,
    pub err: Option<String>,
    pub width: u16,
    pub height: u16,
    pub spin_frame: usize,

    next_id: u64,
    /// Outstanding page request (initial load or load-more).
    page_req: Option<u64>,
    /// Outstanding check.
    check_req: Option<u64>,
    pub(super) clips: RefCell<HashMap<(String, i64), Clipped>>,
}

impl FeedApp {
    pub fn new(
        theme: Theme,
        refresh: Duration,
        fancy: bool,
        me_did: &str,
        me_handle: &str,
    ) -> Self {
        FeedApp {
            theme,
            fancy,
            refresh,
            me_did: me_did.into(),
            me_handle: me_handle.into(),
            decisions: HashMap::new(),
            items: Vec::new(),
            cur: 0,
            list_top: 0,
            tl_cursor: String::new(),
            follow_loading: true,
            more_busy: false,
            new_posts: Vec::new(),
            new_gap: false,
            checking: false,
            apply_when_done: false,
            refresh_err: None,
            fresh: HashMap::new(),
            animating: false,
            in_post: false,
            composing: false,
            busy: false,
            status: String::new(),
            err: None,
            width: 0,
            height: 0,
            spin_frame: 0,
            next_id: 0,
            page_req: None,
            check_req: None,
            clips: RefCell::new(HashMap::new()),
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Effects to run at startup: load the first timeline page.
    pub fn start(&mut self) -> Vec<Effect> {
        let id = self.id();
        self.page_req = Some(id);
        vec![Effect::LoadPage {
            id,
            cursor: String::new(),
        }]
    }

    /// Whether a spinner is on screen (so the runtime keeps its timer running).
    pub fn needs_spin(&self) -> bool {
        self.follow_loading
    }

    // ------------------------------------------------------------------ layout

    pub fn list_width(&self) -> i64 {
        if self.width == 0 {
            110
        } else {
            (self.width as i64 - 2).max(40)
        }
    }

    pub fn hint_items(&self) -> Vec<String> {
        [
            "↑ ↓ move",
            "PgUp PgDn page",
            "Home End",
            "R refresh",
            "q quit",
        ]
        .map(String::from)
        .to_vec()
    }

    pub fn hint_lines(&self) -> Vec<String> {
        wrap_items(&self.hint_items(), self.width as usize)
    }

    /// Rows available to the list.
    pub fn avail(&self) -> usize {
        if self.height == 0 {
            return 1000;
        }
        (self.height as usize)
            .saturating_sub(6 + self.hint_lines().len() - 1)
            .max(4)
    }

    /// Whether item `i` answers the post right above it (so the list indents it).
    pub fn in_thread(&self, i: usize) -> bool {
        if i == 0 || i >= self.items.len() {
            return false;
        }
        let it = &self.items[i];
        it.post.reply
            && it.by.is_empty()
            && !it.parent.uri.is_empty()
            && it.parent.uri == self.items[i - 1].uri
    }

    /// Wrapped text and quote of an item at body width `bw` (cached per item and width).
    pub(super) fn body_lines(&self, it: &FeedItem, bw: i64) -> Clipped {
        let key = (it.key(), bw);
        if let Some(c) = self.clips.borrow().get(&key) {
            return c.clone();
        }
        let c = Clipped {
            text: clip(&it.post.preview(), bw, PREVIEW_LINES),
            quote: clip(&collapse_ws(&it.post.quote), bw - 2, QUOTE_LINES),
        };
        if !it.uri.is_empty() {
            // without a URI, items cannot be told apart
            self.clips.borrow_mut().insert(key, c.clone());
        }
        c
    }

    /// Rows item `i` occupies: header, text/quote lines (or "(no text)"), blank separator.
    pub fn item_height(&self, i: usize, w: i64) -> usize {
        let gw = if self.in_thread(i) { 4 } else { 0 };
        let c = self.body_lines(&self.items[i], w - 2 - 6 - gw);
        1 + (c.text.len() + c.quote.len()).max(1) + 1
    }

    /// Scrolls the list so the selected item is fully on screen.
    pub fn ensure_visible(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.cur = self.cur.min(self.items.len() - 1);
        self.list_top = self.list_top.min(self.cur);
        // Walk up from the selection to the highest item that still fits with it on screen.
        let (w, room) = (self.list_width(), self.avail());
        let (mut h, mut low) = (0, self.cur);
        for i in (self.list_top..=self.cur).rev() {
            h += self.item_height(i, w);
            if h > room {
                break;
            }
            low = i;
        }
        self.list_top = low;
    }

    /// The reader is on the first post of Following with nothing open or being written, so
    /// new posts can be added without moving anything they read.
    pub fn reading_top(&self) -> bool {
        self.cur == 0 && self.list_top == 0 && !self.in_post && !self.composing && !self.busy
    }

    // ------------------------------------------------------------------ input

    pub fn on_key(&mut self, key: KeyEvent, now: Now) -> Vec<Effect> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('R') => return self.refresh_key(now),
            _ => {}
        }
        if self.busy {
            return Vec::new();
        }
        let rows = (self.avail() / (PREVIEW_LINES + 2)).max(1);
        let last = self.items.len().saturating_sub(1);
        let target = match key.code {
            KeyCode::Up => Some(self.cur.saturating_sub(1)),
            KeyCode::Down => Some(self.cur + 1),
            KeyCode::PageUp => Some(self.cur.saturating_sub(rows)),
            KeyCode::PageDown => Some(self.cur + rows),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(last),
            _ => None,
        };
        if let Some(i) = target {
            self.cur = i.min(last);
            self.status.clear();
            self.err = None;
            self.ensure_visible();
        }
        self.maybe_load_more()
    }

    /// `R` jumps to the top, where new posts land: insert what the header announced, or
    /// start a check (or ask an already-running one to apply when it finishes).
    fn refresh_key(&mut self, now: Now) -> Vec<Effect> {
        self.cur = 0;
        self.list_top = 0;
        self.in_post = false;
        if !self.new_posts.is_empty() {
            return self.apply_pending(now);
        }
        self.start_check(true)
    }

    /// Fetches the next timeline page when the cursor nears the end.
    fn maybe_load_more(&mut self) -> Vec<Effect> {
        if self.tl_cursor.is_empty()
            || self.more_busy
            || self.follow_loading
            || self.items.len().saturating_sub(self.cur) > LOAD_MORE_WITHIN
        {
            return Vec::new();
        }
        self.more_busy = true;
        let id = self.id();
        self.page_req = Some(id);
        vec![Effect::LoadPage {
            id,
            cursor: self.tl_cursor.clone(),
        }]
    }

    // ------------------------------------------------------------------ checks

    /// Starts a read-only check for posts newer than what is shown or pending.
    fn start_check(&mut self, manual: bool) -> Vec<Effect> {
        if self.follow_loading {
            if manual {
                self.status = "still loading the timeline".into();
            }
            return Vec::new();
        }
        if self.checking {
            if manual {
                // a check is already running: show its result as soon as it lands
                self.apply_when_done = true;
                self.status = "checking for new posts…".into();
            }
            return Vec::new();
        }
        let known: HashSet<String> = self
            .items
            .iter()
            .chain(self.new_posts.iter())
            .map(FeedItem::key)
            .collect();
        self.checking = true;
        if manual {
            self.status = "checking for new posts…".into();
        }
        let id = self.id();
        self.check_req = Some(id);
        vec![Effect::Check { id, manual, known }]
    }

    /// Inserts the held-back posts at the top. The post being read stays selected, unless
    /// the reader is at the very top, where the newest post becomes the selection.
    fn apply_pending(&mut self, now: Now) -> Vec<Effect> {
        let n = self.new_posts.len();
        if n == 0 {
            self.status = "no new posts".into();
            return Vec::new();
        }
        let at_top = self.cur == 0 && self.list_top == 0 && !self.in_post;
        let mut items = std::mem::take(&mut self.new_posts);
        if self.fancy {
            for it in &items {
                self.fresh.insert(it.key(), now.mono);
            }
        }
        items.append(&mut self.items);
        self.items = items;
        if !at_top {
            self.cur += n;
            self.list_top += n;
        }
        self.status = if n == 1 {
            "1 new post".into()
        } else {
            format!("{n} new posts")
        };
        if self.new_gap {
            self.status
                .push_str(" (more arrived than could be fetched; the newest are shown)");
        }
        self.new_gap = false;
        self.ensure_visible();
        self.schedule_anim()
    }

    fn schedule_anim(&mut self) -> Vec<Effect> {
        if self.fresh.is_empty() || self.animating {
            return Vec::new();
        }
        self.animating = true;
        vec![Effect::ScheduleAnim]
    }

    /// Highlight style for a post inserted at `t`, or `None` once it has faded.
    pub fn fresh_color(&self, key: &str, now: Now) -> Option<ratatui::style::Color> {
        let t = self.fresh.get(key)?;
        let age = now.mono.saturating_duration_since(*t);
        if age >= FRESH_FOR || self.theme.fresh.is_empty() || !self.theme.color {
            return None;
        }
        let n = self.theme.fresh.len();
        let i = ((age.as_millis() * n as u128) / FRESH_FOR.as_millis()) as usize;
        Some(self.theme.fresh[i.min(n - 1)])
    }

    /// Whether `key` is still inside its highlight window (even if the theme has no colour).
    pub fn is_fresh(&self, key: &str, now: Now) -> bool {
        self.fresh
            .get(key)
            .is_some_and(|t| now.mono.saturating_duration_since(*t) < FRESH_FOR)
    }

    // ------------------------------------------------------------------ results

    pub fn on_msg(&mut self, msg: Msg, now: Now) -> Vec<Effect> {
        match msg {
            Msg::Resize(w, h) => {
                if w != self.width {
                    self.clips.borrow_mut().clear(); // wrapped for the old width
                }
                self.width = w;
                self.height = h;
                self.ensure_visible();
                Vec::new()
            }
            Msg::SpinTick => {
                self.spin_frame = self.spin_frame.wrapping_add(1);
                Vec::new()
            }
            Msg::AuditFailed(e) => {
                if self.err.is_none() {
                    self.err = Some(format!("audit log: {e}"));
                }
                Vec::new()
            }
            Msg::AnimTick => {
                self.animating = false;
                self.fresh
                    .retain(|_, t| now.mono.saturating_duration_since(*t) < FRESH_FOR);
                self.schedule_anim()
            }
            Msg::RefreshTick => self.start_check(false),
            Msg::Page { id, result } => {
                if self.page_req != Some(id) {
                    return Vec::new(); // stale: superseded or already answered
                }
                self.page_req = None;
                self.follow_loading = false;
                self.more_busy = false;
                match result {
                    Err(e) => self.err = Some(e),
                    Ok((items, cursor)) => {
                        self.items.extend(items);
                        self.tl_cursor = cursor;
                        self.ensure_visible();
                    }
                }
                Vec::new()
            }
            Msg::Check { id, manual, result } => {
                if self.check_req != Some(id) {
                    return Vec::new();
                }
                self.check_req = None;
                self.on_check(manual, result, now)
            }
        }
    }

    fn on_check(
        &mut self,
        manual: bool,
        result: Result<CheckOutcome, String>,
        now: Now,
    ) -> Vec<Effect> {
        self.checking = false;
        let manual = manual || self.apply_when_done;
        self.apply_when_done = false;
        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                let mut effects = Vec::new();
                // A background check retries every interval: audit a failure when it starts
                // or changes, not once per tick while the same problem persists. Manual
                // failures are always audited.
                let repeat = self.refresh_err.as_deref() == Some(e.as_str());
                if manual || !repeat {
                    effects.push(Effect::Audit(Box::new(AuditEvent {
                        action: "refresh".into(),
                        did: self.me_did.clone(),
                        handle: self.me_handle.clone(),
                        source: "tui:following".into(),
                        detail: if manual {
                            "manual refresh (R)"
                        } else {
                            "background check"
                        }
                        .into(),
                        error: e.clone(),
                        ..Default::default()
                    })));
                }
                if manual {
                    self.status.clear();
                    self.err = Some(format!("refresh: {e}"));
                } else {
                    self.refresh_err = Some(e);
                }
                return effects;
            }
        };
        self.refresh_err = None;
        // Deduplicate against what is shown and what is already pending.
        let have: HashSet<String> = self
            .items
            .iter()
            .chain(self.new_posts.iter())
            .map(FeedItem::key)
            .collect();
        let mut seen = HashSet::new();
        let mut fresh: Vec<FeedItem> = outcome
            .items
            .into_iter()
            .filter(|it| !have.contains(&it.key()) && seen.insert(it.key()))
            .collect();
        fresh.append(&mut self.new_posts);
        self.new_posts = fresh;
        self.new_gap |= outcome.gap;
        if manual || self.reading_top() {
            if manual {
                // R means "take me to the top", even if you moved while the check ran
                self.cur = 0;
                self.list_top = 0;
                self.in_post = false;
            }
            return self.apply_pending(now);
        }
        Vec::new()
    }
}

/// Lays hint items out on one line if they fit in `width`, otherwise wraps them.
pub fn wrap_items(items: &[String], width: usize) -> Vec<String> {
    if width == 0 {
        return vec![items.join(" · ")];
    }
    for sep in [" · ", "  "] {
        let h = items.join(sep);
        if crate::termtext::width(&h) <= width {
            return vec![h];
        }
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for it in items {
        if cur.is_empty() {
            cur = it.clone();
        } else if crate::termtext::width(&cur) + 2 + crate::termtext::width(it) <= width {
            cur.push_str("  ");
            cur.push_str(it);
        } else {
            lines.push(std::mem::replace(&mut cur, it.clone()));
        }
    }
    lines.push(cur);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    pub(crate) fn item(rkey: &str, by: &str) -> FeedItem {
        let mut it = FeedItem {
            uri: format!("at://did:x/app.bsky.feed.post/{rkey}"),
            by: by.into(),
            ..Default::default()
        };
        it.post.text = format!("post {rkey}");
        it.author.handle = "x.test".into();
        it
    }

    fn app() -> FeedApp {
        let mut a = FeedApp::new(
            Theme::named("dark", true, true),
            Duration::from_secs(30),
            true,
            "did:me",
            "me.test",
        );
        a.width = 100;
        a.height = 40;
        a
    }

    fn loaded(n: usize) -> FeedApp {
        let mut a = app();
        let eff = a.start();
        let Effect::LoadPage { id, .. } = eff[0].clone() else {
            panic!()
        };
        let items = (0..n).map(|i| item(&format!("old{i}"), "")).collect();
        a.on_msg(
            Msg::Page {
                id,
                result: Ok((items, "next".into())),
            },
            Now::current(),
        );
        a
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn check_id(eff: &[Effect]) -> u64 {
        match eff {
            [Effect::Check { id, .. }] => *id,
            other => panic!("expected a check, got {other:?}"),
        }
    }

    fn outcome(rkeys: &[&str], gap: bool) -> CheckOutcome {
        CheckOutcome {
            items: rkeys.iter().map(|k| item(k, "")).collect(),
            gap,
        }
    }

    #[test]
    fn initial_load_then_ignores_stale_page() {
        let mut a = app();
        let Effect::LoadPage { id, cursor } = a.start().remove(0) else {
            panic!()
        };
        assert_eq!(cursor, "");
        assert!(a.follow_loading && a.needs_spin());
        a.on_msg(
            Msg::Page {
                id: id + 99,
                result: Ok((vec![item("x", "")], String::new())),
            },
            Now::current(),
        );
        assert!(
            a.items.is_empty() && a.follow_loading,
            "unknown id is ignored"
        );
        a.on_msg(
            Msg::Page {
                id,
                result: Ok((vec![item("a", ""), item("b", "")], "c".into())),
            },
            Now::current(),
        );
        assert_eq!(
            (a.items.len(), a.tl_cursor.as_str(), a.follow_loading),
            (2, "c", false)
        );
        a.on_msg(
            Msg::Page {
                id,
                result: Ok((vec![item("late", "")], String::new())),
            },
            Now::current(),
        );
        assert_eq!(a.items.len(), 2, "answered ids are ignored");
    }

    #[test]
    fn navigation_clamps_and_clears_messages() {
        let mut a = loaded(5);
        a.status = "s".into();
        a.err = Some("e".into());
        a.on_key(key(KeyCode::Up), Now::current());
        assert_eq!(a.cur, 0);
        assert!(a.status.is_empty() && a.err.is_none());
        a.on_key(key(KeyCode::Down), Now::current());
        a.on_key(key(KeyCode::Down), Now::current());
        assert_eq!(a.cur, 2);
        a.on_key(key(KeyCode::End), Now::current());
        assert_eq!(a.cur, 4);
        a.on_key(key(KeyCode::Down), Now::current());
        assert_eq!(a.cur, 4);
        a.on_key(key(KeyCode::Home), Now::current());
        assert_eq!(a.cur, 0);
    }

    #[test]
    fn load_more_near_the_end_only_once() {
        let mut a = loaded(20);
        assert!(
            a.on_key(key(KeyCode::Down), Now::current()).is_empty(),
            "far from the end"
        );
        a.cur = 12;
        let eff = a.on_key(key(KeyCode::Down), Now::current());
        let [Effect::LoadPage { id, cursor }] = &eff[..] else {
            panic!("{eff:?}")
        };
        assert_eq!(cursor, "next");
        assert!(a.more_busy);
        assert!(
            a.on_key(key(KeyCode::Down), Now::current()).is_empty(),
            "no overlapping load"
        );
        let id = *id;
        a.on_msg(
            Msg::Page {
                id,
                result: Ok((
                    (0..5).map(|i| item(&format!("p2-{i}"), "")).collect(),
                    String::new(),
                )),
            },
            Now::current(),
        );
        assert_eq!(
            (a.items.len(), a.more_busy, a.tl_cursor.as_str()),
            (25, false, "")
        );
        a.cur = 24;
        assert!(
            a.on_key(key(KeyCode::Down), Now::current()).is_empty(),
            "cursor ended"
        );
    }

    #[test]
    fn quit_keys() {
        let mut a = loaded(1);
        assert_eq!(
            a.on_key(key(KeyCode::Char('q')), Now::current()),
            [Effect::Quit]
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(a.on_key(ctrl_c, Now::current()), [Effect::Quit]);
    }

    #[test]
    fn background_result_inserts_immediately_at_the_top() {
        let mut a = loaded(3);
        let now = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        assert!(a.checking);
        let eff = a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n2", "n1"], false)),
            },
            now,
        );
        assert_eq!(
            a.items
                .iter()
                .map(|i| i.uri.rsplit('/').next().unwrap())
                .collect::<Vec<_>>(),
            ["n2", "n1", "old0", "old1", "old2"]
        );
        assert_eq!(a.cur, 0);
        assert_eq!(a.status, "2 new posts");
        assert_eq!(
            eff,
            [Effect::ScheduleAnim],
            "fancy mode animates the highlight"
        );
        assert!(a.new_posts.is_empty() && !a.checking);
    }

    #[test]
    fn background_result_is_held_when_reading_elsewhere() {
        let mut a = loaded(10);
        let now = Now::current();
        a.on_key(key(KeyCode::Down), now);
        a.on_key(key(KeyCode::Down), now);
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n1"], false)),
            },
            now,
        );
        assert_eq!(
            (a.items.len(), a.new_posts.len(), a.cur),
            (10, 1, 2),
            "held, selection untouched"
        );
        // a second check finds another post: the pending list grows, newest first
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n2"], false)),
            },
            now,
        );
        assert_eq!(
            a.new_posts
                .iter()
                .map(|i| i.uri.rsplit('/').next().unwrap())
                .collect::<Vec<_>>(),
            ["n2", "n1"]
        );
        // R inserts without another fetch and jumps to the top
        let eff = a.on_key(key(KeyCode::Char('R')), now);
        assert_eq!(eff, [Effect::ScheduleAnim]);
        assert_eq!(
            (a.items.len(), a.cur, a.list_top, a.new_posts.len()),
            (12, 0, 0, 0)
        );
        assert_eq!(a.items[0].uri.rsplit('/').next(), Some("n2"));
        assert_eq!(a.status, "2 new posts");
    }

    #[test]
    fn insertion_is_blocked_while_detail_or_composer_is_open() {
        let mut a = loaded(3);
        let now = Now::current();
        for blocker in ["in_post", "composing", "busy"] {
            let id = check_id(&a.on_msg(Msg::RefreshTick, now));
            match blocker {
                "in_post" => a.in_post = true,
                "composing" => a.composing = true,
                _ => a.busy = true,
            }
            a.on_msg(
                Msg::Check {
                    id,
                    manual: false,
                    result: Ok(outcome(&[&format!("b-{blocker}")], false)),
                },
                now,
            );
            assert_eq!(a.new_posts.len(), 1, "{blocker} holds the post back");
            (a.in_post, a.composing, a.busy) = (false, false, false);
            a.new_posts.clear();
        }
    }

    #[test]
    fn dedupes_against_displayed_pending_and_within_the_result() {
        let mut a = loaded(2);
        let now = Now::current();
        a.cur = 1; // reading elsewhere: results stay pending
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n1", "n1", "old0"], false)),
            },
            now,
        );
        assert_eq!(
            a.new_posts.len(),
            1,
            "duplicate in result and displayed item dropped"
        );
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n1", "n0"], false)),
            },
            now,
        );
        assert_eq!(a.new_posts.len(), 2, "already-pending item dropped");
        // identity is URI + reposter: the same post from a second reposter is a new entry
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        let mut o = outcome(&[], false);
        o.items.push(item("n0", "r2.test"));
        o.items.push(item("n0", ""));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(o),
            },
            now,
        );
        assert_eq!(a.new_posts.len(), 3);
        assert_eq!(a.new_posts[0].by, "r2.test");
    }

    #[test]
    fn checks_know_displayed_and_pending_keys() {
        let mut a = loaded(2);
        a.cur = 1;
        let now = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["p"], false)),
            },
            now,
        );
        let eff = a.on_msg(Msg::RefreshTick, now);
        let [Effect::Check { known, .. }] = &eff[..] else {
            panic!()
        };
        assert_eq!(known.len(), 3);
        assert!(known.contains("at://did:x/app.bsky.feed.post/p|"));
    }

    #[test]
    fn gap_is_reported_when_inserting() {
        let mut a = loaded(2);
        let now = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n"], true)),
            },
            now,
        );
        assert_eq!(
            a.status,
            "1 new post (more arrived than could be fetched; the newest are shown)"
        );
        assert!(!a.new_gap);
    }

    #[test]
    fn r_during_a_check_applies_when_it_lands_and_never_overlaps() {
        let mut a = loaded(4);
        let now = Now::current();
        a.cur = 2;
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        assert!(
            a.on_msg(Msg::RefreshTick, now).is_empty(),
            "no second check while one runs"
        );
        assert!(a.on_key(key(KeyCode::Char('R')), now).is_empty());
        assert!(a.apply_when_done && a.status == "checking for new posts…");
        assert_eq!(a.cur, 0, "R jumps to the top immediately");
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n"], false)),
            },
            now,
        );
        assert_eq!((a.items.len(), a.cur, a.new_posts.len()), (5, 0, 0));
        assert!(!a.apply_when_done);
    }

    #[test]
    fn r_with_nothing_waiting_starts_a_manual_check_and_reports_none() {
        let mut a = loaded(2);
        let now = Now::current();
        let id = check_id(&a.on_key(key(KeyCode::Char('R')), now));
        assert_eq!(a.status, "checking for new posts…");
        a.on_msg(
            Msg::Check {
                id,
                manual: true,
                result: Ok(outcome(&[], false)),
            },
            now,
        );
        assert_eq!(a.status, "no new posts");
    }

    #[test]
    fn r_while_the_timeline_is_still_loading_does_nothing() {
        let mut a = app();
        a.start();
        assert!(a.on_key(key(KeyCode::Char('R')), Now::current()).is_empty());
        assert_eq!(a.status, "still loading the timeline");
    }

    #[test]
    fn stale_check_results_are_ignored() {
        let mut a = loaded(2);
        let now = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id: id + 1,
                manual: false,
                result: Ok(outcome(&["n"], false)),
            },
            now,
        );
        assert!(a.checking && a.items.len() == 2);
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&[], false)),
            },
            now,
        );
        assert!(!a.checking);
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["dup"], false)),
            },
            now,
        );
        assert_eq!(a.items.len(), 2, "a repeated answer is ignored");
    }

    #[test]
    fn background_failure_warns_audits_once_and_clears_on_success() {
        let mut a = loaded(2);
        let now = Now::current();
        let fail = |a: &mut FeedApp, msg: &str| {
            let id = check_id(&a.on_msg(Msg::RefreshTick, now));
            a.on_msg(
                Msg::Check {
                    id,
                    manual: false,
                    result: Err(msg.into()),
                },
                now,
            )
        };
        let eff = fail(&mut a, "getTimeline: HTTP 502");
        let [Effect::Audit(ev)] = &eff[..] else {
            panic!("{eff:?}")
        };
        assert_eq!(
            (ev.action.as_str(), ev.source.as_str(), ev.detail.as_str()),
            ("refresh", "tui:following", "background check")
        );
        assert_eq!(
            (
                ev.did.as_str(),
                ev.handle.as_str(),
                ev.error.as_str(),
                ev.ok
            ),
            ("did:me", "me.test", "getTimeline: HTTP 502", false)
        );
        assert_eq!(a.refresh_err.as_deref(), Some("getTimeline: HTTP 502"));
        assert!(
            a.err.is_none(),
            "a background failure is only a header warning"
        );
        assert!(
            fail(&mut a, "getTimeline: HTTP 502").is_empty(),
            "same error is not audited again"
        );
        assert_eq!(
            fail(&mut a, "getTimeline: HTTP 500").len(),
            1,
            "a different error is"
        );
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&[], false)),
            },
            now,
        );
        assert!(a.refresh_err.is_none());
        assert_eq!(
            fail(&mut a, "getTimeline: HTTP 500").len(),
            1,
            "audited again after it cleared"
        );
    }

    #[test]
    fn manual_failure_is_shown_and_always_audited() {
        let mut a = loaded(2);
        let now = Now::current();
        for _ in 0..2 {
            let id = check_id(&a.on_key(key(KeyCode::Char('R')), now));
            let eff = a.on_msg(
                Msg::Check {
                    id,
                    manual: true,
                    result: Err("boom".into()),
                },
                now,
            );
            let [Effect::Audit(ev)] = &eff[..] else {
                panic!("{eff:?}")
            };
            assert_eq!(ev.detail, "manual refresh (R)");
            assert_eq!(a.err.as_deref(), Some("refresh: boom"));
            assert!(a.status.is_empty());
        }
        assert!(
            a.refresh_err.is_none(),
            "a manual failure does not set the background warning"
        );
    }

    #[test]
    fn audit_write_failure_is_surfaced_once() {
        let mut a = loaded(1);
        a.on_msg(Msg::AuditFailed("disk full".into()), Now::current());
        a.on_msg(Msg::AuditFailed("other".into()), Now::current());
        assert_eq!(a.err.as_deref(), Some("audit log: disk full"));
    }

    #[test]
    fn fancy_highlight_fades_over_two_seconds_with_80ms_ticks() {
        let mut a = loaded(2);
        let t0 = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, t0));
        a.on_msg(
            Msg::Check {
                id,
                manual: false,
                result: Ok(outcome(&["n"], false)),
            },
            t0,
        );
        let key = a.items[0].key();
        assert!(a.is_fresh(&key, t0) && a.animating);
        let first = a.fresh_color(&key, t0).unwrap();
        let last = a
            .fresh_color(&key, t0.plus(Duration::from_millis(1990)))
            .unwrap();
        assert_ne!(first, last, "colour steps along the ramp");
        assert_eq!(first, a.theme.fresh[0]);
        assert_eq!(last, *a.theme.fresh.last().unwrap());
        // ticking keeps scheduling until the highlight expires
        assert_eq!(
            a.on_msg(Msg::AnimTick, t0.plus(ANIM_TICK)),
            [Effect::ScheduleAnim]
        );
        assert!(
            a.on_msg(Msg::AnimTick, t0.plus(Duration::from_millis(2100)))
                .is_empty()
        );
        assert!(a.fresh.is_empty() && !a.animating);
        assert!(
            a.fresh_color(&key, t0.plus(Duration::from_secs(3)))
                .is_none()
        );
    }

    #[test]
    fn plain_mode_does_not_animate() {
        let mut a = loaded(2);
        a.fancy = false;
        let now = Now::current();
        let id = check_id(&a.on_msg(Msg::RefreshTick, now));
        assert!(
            a.on_msg(
                Msg::Check {
                    id,
                    manual: false,
                    result: Ok(outcome(&["n"], false))
                },
                now
            )
            .is_empty()
        );
        assert!(a.fresh.is_empty());
    }

    #[test]
    fn resize_rewraps_and_keeps_selection_visible() {
        let mut a = loaded(30);
        a.cur = 20;
        a.on_msg(Msg::Resize(100, 20), Now::current());
        assert!(a.list_top <= a.cur && a.list_top > 0);
        let used: usize = (a.list_top..=a.cur)
            .map(|i| a.item_height(i, a.list_width()))
            .sum();
        assert!(used <= a.avail());
    }

    #[test]
    fn hints_wrap_on_narrow_terminals() {
        let items: Vec<String> = ["aaaa", "bbbb", "cccc"].map(String::from).to_vec();
        assert_eq!(wrap_items(&items, 100), ["aaaa · bbbb · cccc"]);
        assert_eq!(wrap_items(&items, 12), ["aaaa  bbbb", "cccc"]);
        assert_eq!(wrap_items(&items, 0), ["aaaa · bbbb · cccc"]);
    }
}
