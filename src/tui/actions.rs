//! Moderation actions in the feed view: block / follow / keep / unfollow / undo.
//!
//! Port of `decide`, `undo`, `unfollowKey`, `applyAct` and the audit helpers from Go
//! `internal/tui/ui.go`. The remote half runs in the runtime ([`run_act`]); the local half
//! (state file, history, audit) runs in [`FeedApp::apply_act`], so the remote result and the
//! local save are modelled separately: when the remote action succeeds but saving fails the
//! result is kept as `pending`, further actions and quitting wait, and `r` retries *only*
//! the save.

use ratatui::style::Style;

use super::feed::{Effect, FeedApp, ProfileSlot};
use crate::app::{create_for, delete_for, is_remote, viewer_uri};
use crate::bsky::{Client, Profile};
use crate::store::{self, AuditEvent, Entry, Snapshot};

/// What is needed to undo (or describe) one decision.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoEntry {
    /// Timeline index the decision was made on.
    pub idx: usize,
    pub profile: Profile,
    pub decision: String,
    /// The block/follow record this decision created.
    pub record: String,
    /// The decision recorded before this one.
    pub prev: Option<Entry>,
}

/// A remote action to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum ActRequest {
    /// Record `entry.decision`; `lift` says the previous block/follow must be deleted first
    /// (`known` is its record, or empty to look it up).
    Decide {
        entry: UndoEntry,
        lift: bool,
        known: String,
    },
    Undo(UndoEntry),
    Unfollow(UndoEntry),
}

/// The outcome of a remote action, awaiting its local save.
#[derive(Debug, Clone, PartialEq)]
pub struct ActMsg {
    pub undo: UndoEntry,
    pub is_undo: bool,
    pub is_unfollow: bool,
    /// Block/follow record created by this action, if any.
    pub new_uri: String,
    /// An earlier block/follow record was deleted on the way.
    pub lifted: bool,
    pub err: Option<String>,
}

impl ActMsg {
    fn new(undo: UndoEntry) -> Self {
        ActMsg {
            undo,
            is_undo: false,
            is_unfollow: false,
            new_uri: String::new(),
            lifted: false,
            err: None,
        }
    }
}

/// Performs the remote side of `req`. Never touches local state.
pub async fn run_act(c: &Client, req: ActRequest) -> ActMsg {
    match req {
        ActRequest::Decide {
            entry,
            lift,
            mut known,
        } => {
            let mut msg = ActMsg::new(entry.clone());
            let (did, decision) = (entry.profile.did.as_str(), entry.decision.as_str());
            if lift {
                // e.g. keep/skip on a blocked account lifts the block
                let prev = entry
                    .prev
                    .as_ref()
                    .map(|p| p.decision.as_str())
                    .unwrap_or_default();
                if known.is_empty()
                    && let Ok(fp) = c.get_profile(did).await
                {
                    known = viewer_uri(&fp, prev).to_string();
                }
                if !known.is_empty()
                    && let Err(e) = delete_for(c, prev, &known).await
                {
                    msg.err = Some(e.to_string());
                    return msg;
                }
                msg.lifted = true;
            }
            if is_remote(decision) {
                match create_for(c, decision, did).await {
                    Ok(uri) => msg.new_uri = uri,
                    Err(e) => msg.err = Some(e.to_string()),
                }
            }
            msg
        }
        ActRequest::Undo(e) => {
            let mut msg = ActMsg {
                is_undo: true,
                ..ActMsg::new(e.clone())
            };
            if is_remote(&e.decision)
                && !e.record.is_empty()
                && let Err(err) = delete_for(c, &e.decision, &e.record).await
            {
                msg.err = Some(err.to_string());
            }
            if let Some(prev) = &e.prev
                && msg.err.is_none()
                && is_remote(&prev.decision)
                && prev.decision != e.decision
            {
                // restore what we lifted
                match create_for(c, &prev.decision, &e.profile.did).await {
                    Ok(uri) => msg.new_uri = uri,
                    Err(err) => msg.err = Some(err.to_string()),
                }
            }
            msg
        }
        ActRequest::Unfollow(mut e) => {
            if e.record.is_empty() {
                match c.get_profile(&e.profile.did).await {
                    Ok(p) => e.record = p.viewer.following,
                    Err(err) => {
                        return ActMsg {
                            is_unfollow: true,
                            err: Some(err.to_string()),
                            ..ActMsg::new(e)
                        };
                    }
                }
            }
            let mut msg = ActMsg {
                is_unfollow: true,
                ..ActMsg::new(e.clone())
            };
            if e.record.is_empty() {
                msg.err = Some(format!("you do not follow @{}", e.profile.handle));
            } else if let Err(err) = c.unfollow(&e.record).await {
                msg.err = Some(err.to_string());
            }
            msg
        }
    }
}

fn decision_action(decision: &str) -> &'static str {
    match decision {
        store::BLOCKED => "block",
        store::FOLLOWED => "follow",
        store::KEPT => "keep",
        store::SKIPPED => "skip",
        _ => "",
    }
}

/// The audit event for deleting the record behind an overridden decision.
fn lift_action(decision: &str) -> &'static str {
    if decision == store::FOLLOWED {
        "unfollow"
    } else {
        "unblock"
    }
}

impl FeedApp {
    /// The account being looked at: the author of the current post.
    fn acting_subject(&self) -> Option<Profile> {
        self.subject()
    }

    fn start_busy(&mut self) {
        self.busy = true;
        self.err = None;
    }

    /// `b`, `f` and `k`. Blocking someone you follow asks for a second press first.
    pub(super) fn decision_key(&mut self, key: char, confirm: &str) -> Vec<Effect> {
        let Some(p) = self.acting_subject() else {
            return Vec::new();
        };
        match key {
            'k' => self.decide_key(p, store::KEPT),
            'f' => self.decide_key(p, store::FOLLOWED),
            _ => {
                if p.did == self.me_did {
                    self.status = "that's you".into();
                    return Vec::new();
                }
                if !p.viewer.following.is_empty() && confirm != p.did {
                    self.confirm = p.did.clone();
                    self.status = format!("You follow @{}. Press b again to block them.", p.handle);
                    return Vec::new();
                }
                self.decide_key(p, store::BLOCKED)
            }
        }
    }

    /// Applies `decision` to `p`, unless that is already recorded.
    fn decide_key(&mut self, p: Profile, decision: &str) -> Vec<Effect> {
        if self
            .state
            .entries
            .get(&p.did)
            .is_some_and(|e| e.decision == decision)
        {
            self.status = format!("@{} is already {decision}", p.handle);
            return Vec::new();
        }
        if decision == store::FOLLOWED && !p.viewer.following.is_empty() {
            self.status = format!("you already follow @{}", p.handle);
            return Vec::new();
        }
        self.decide(p, decision)
    }

    /// Records `decision` for `p`. Blocking or following (and lifting an earlier block or
    /// follow when it is overridden) calls the API first; the state file is only touched in
    /// [`FeedApp::apply_act`].
    fn decide(&mut self, p: Profile, decision: &str) -> Vec<Effect> {
        let prev = self.state.entries.get(&p.did).cloned();
        let lift = prev
            .as_ref()
            .is_some_and(|e| is_remote(&e.decision) && e.decision != decision);
        let mut known = String::new();
        if lift {
            let prev_decision = prev
                .as_ref()
                .map(|e| e.decision.as_str())
                .unwrap_or_default();
            known = self.records.get(&p.did).cloned().unwrap_or_default();
            if known.is_empty()
                && let Some(ProfileSlot::Loaded(fp)) = self.profiles.get(&p.did)
            {
                known = viewer_uri(fp, prev_decision).to_string();
            }
        }
        let entry = UndoEntry {
            idx: self.cur,
            profile: p,
            decision: decision.into(),
            record: String::new(),
            prev,
        };
        self.start_busy();
        vec![Effect::Act(Box::new(ActRequest::Decide {
            entry,
            lift,
            known,
        }))]
    }

    /// `u`: unfollow the current author after a second press.
    pub(super) fn unfollow_key(&mut self, confirm: &str) -> Vec<Effect> {
        let Some(p) = self.acting_subject() else {
            return Vec::new();
        };
        let want = format!("unfollow:{}", p.did);
        if confirm != want {
            self.confirm = want;
            self.status = format!("Unfollow @{}? Press u again to confirm.", p.handle);
            return Vec::new();
        }
        let mut record = p.viewer.following.clone();
        if record.is_empty() && self.state.decision(&p.did) == store::FOLLOWED {
            record = self.records.get(&p.did).cloned().unwrap_or_default();
        }
        let entry = UndoEntry {
            idx: self.cur,
            profile: p,
            decision: store::FOLLOWED.into(),
            record,
            prev: None,
        };
        self.start_busy();
        self.status.clear();
        vec![Effect::Act(Box::new(ActRequest::Unfollow(entry)))]
    }

    /// `U`: reverts the latest decision.
    pub(super) fn undo_key(&mut self) -> Vec<Effect> {
        let Some(e) = self.history.pop() else {
            self.status = "nothing to undo".into();
            return Vec::new();
        };
        self.start_busy();
        vec![Effect::Act(Box::new(ActRequest::Undo(e)))]
    }

    /// Records the outcome of a finished remote action: all state writes happen here.
    pub fn apply_act(&mut self, msg: ActMsg) -> Vec<Effect> {
        self.busy = false;
        let e = msg.undo.clone();
        let did = e.profile.did.clone();
        let mut effects = Vec::new();
        if let Some(err) = &msg.err {
            self.err = Some(err.clone());
            if msg.is_undo {
                self.history.push(e); // still undoable
            }
            if msg.lifted {
                self.records.remove(&did); // that record is gone; look it up afresh next time
            }
            effects.push(self.audit_effect(self.audit_event(&msg)));
            self.quitting = false; // show the failure before allowing another quit
            return effects;
        }
        let saved = if msg.is_unfollow {
            if self.state.decision(&did) == store::FOLLOWED {
                self.state.delete(&did)
            } else {
                Ok(())
            }
        } else if msg.is_undo {
            match &e.prev {
                Some(prev) => self.state.put(&did, prev.clone()),
                None => self.state.delete(&did),
            }
        } else {
            self.state.set(&did, &e.profile.handle, &e.decision)
        };
        if let Err(err) = saved {
            let text = format!("save decision: {err} (press r to retry saving)");
            self.pending = Some(msg.clone());
            self.quitting = false;
            self.err = Some(text.clone());
            self.status = "action completed; decision has not been saved".into();
            let mut ev = self.audit_event(&msg);
            ev.ok = false;
            ev.error = text;
            ev.detail = "action completed; local persistence failed".into();
            effects.push(self.audit_effect(ev));
            return effects;
        }
        self.pending = None;
        if msg.is_unfollow {
            self.records.remove(&did);
            self.history.retain(|h| h.profile.did != did);
            for it in self.items.iter_mut().filter(|it| it.author.did == did) {
                it.author.viewer.following.clear();
            }
            if let Some(ProfileSlot::Loaded(p)) = self.profiles.get_mut(&did) {
                p.viewer.following.clear();
            }
            self.status = format!("unfollowed @{}", e.profile.handle);
        } else if msg.is_undo {
            self.records.remove(&did);
            if !msg.new_uri.is_empty() {
                self.records.insert(did.clone(), msg.new_uri.clone());
                // Restoring a lifted block/follow creates a different record. The earlier
                // decision must undo that new record, not the deleted one.
                if let Some(prev) = &e.prev
                    && let Some(h) = self
                        .history
                        .iter_mut()
                        .rev()
                        .find(|h| h.profile.did == did && h.decision == prev.decision)
                {
                    h.record = msg.new_uri.clone();
                }
            }
            self.status = format!("undid {} for @{}", e.decision, e.profile.handle);
        } else {
            self.records.remove(&did);
            if is_remote(&e.decision) {
                self.records.insert(did.clone(), msg.new_uri.clone());
            }
            let mut done = e.clone();
            done.record = msg.new_uri.clone();
            self.history.push(done);
            self.status = format!("{} @{}", e.decision, e.profile.handle);
        }
        self.err = None;
        for ev in self.audit_events(&msg) {
            effects.push(self.audit_effect(ev));
        }
        if self.quitting {
            effects.push(Effect::Quit);
            return effects;
        }
        effects.extend(self.prefetch());
        effects
    }

    fn audit_effect(&self, ev: AuditEvent) -> Effect {
        Effect::Audit(Box::new(ev))
    }

    /// Who reposted the account into your timeline, when that is how it got here.
    fn via(&self, e: &UndoEntry) -> String {
        match self.items.get(e.idx) {
            Some(it) if it.author.did == e.profile.did => it.by.clone(),
            _ => String::new(),
        }
    }

    /// The base audit event for `msg`, whether it succeeded or failed.
    fn audit_event(&self, msg: &ActMsg) -> AuditEvent {
        let e = &msg.undo;
        let detailed = matches!(
            self.profiles.get(&e.profile.did),
            Some(ProfileSlot::Loaded(_))
        );
        let mut ev = AuditEvent {
            action: decision_action(&e.decision).into(),
            did: e.profile.did.clone(),
            handle: e.profile.handle.clone(),
            source: "tui:following".into(),
            via: self.via(e),
            snapshot: Some(Snapshot::of(&e.profile, detailed)),
            ok: msg.err.is_none(),
            record: msg.new_uri.clone(),
            prev: e
                .prev
                .as_ref()
                .map(|p| p.decision.clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        if let Some(err) = &msg.err {
            ev.error = err.clone();
        }
        if msg.is_undo {
            ev.action = "undo".into();
            ev.detail = format!("undid {}", e.decision);
            if is_remote(&e.decision) {
                ev.record = e.record.clone(); // the block/follow record that was removed
            }
            ev.prev = e.decision.clone();
        }
        if msg.is_unfollow {
            ev.action = "unfollow".into();
            ev.record = e.record.clone();
        }
        ev
    }

    /// What a successful action did. Overriding a block or follow with another decision
    /// deletes that record, which gets its own unblock/unfollow event.
    fn audit_events(&self, msg: &ActMsg) -> Vec<AuditEvent> {
        let e = &msg.undo;
        let main = self.audit_event(msg);
        if let Some(prev) = &e.prev
            && !msg.is_undo
            && !msg.is_unfollow
            && is_remote(&prev.decision)
            && prev.decision != e.decision
        {
            let mut lift = main.clone();
            lift.action = lift_action(&prev.decision).into();
            lift.detail = format!("lifted by {}", e.decision);
            lift.record.clear();
            return vec![lift, main];
        }
        vec![main]
    }

    /// Decision tag shown beside a post's author (blocked / followed / kept).
    pub fn decision_tag(
        &self,
        did: &str,
        th: &super::theme::Theme,
    ) -> Option<(&'static str, Style)> {
        match self.state.decision(did) {
            store::BLOCKED => Some(("blocked", th.err_style())),
            store::FOLLOWED => Some(("followed", th.ok_style())),
            store::KEPT => Some(("kept", th.ok_style())),
            _ => None,
        }
    }
}
