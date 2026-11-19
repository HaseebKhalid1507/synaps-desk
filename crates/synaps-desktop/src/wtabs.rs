//! Workers as app objects, beyond the TUI's tray: a worker in a tab of its
//! own ([`crate::tabs::Kind::Worker`]), a context menu, cancelling one, and
//! a hover card. Only what the v0.11.1 daemon already has: the watch
//! journal, `SubagentSteer`, and `/workers cancel sa_N` (an engine command).
//!
//! - **Who.** `sa_N` alone never names a worker: a cold-started daemon
//!   numbers its workers from 1 again. A worker is its handle and the call
//!   that started it, with its name ([`Who`]); a block whose handle names
//!   a later worker now does nothing, a tab or a menu whose does says it
//!   is gone, and a cancel checks before it goes.
//! - **Clicks.** On a tray row a left click opens the worker in place,
//!   over the session's thread (in a worker tab, which shows one worker,
//!   in its own tab instead); on a worker's block in the transcript
//!   (history) it opens a tab. The middle button, Ctrl/Cmd+click and a
//!   double-click open it in a tab of its own, right after its session's
//!   tab (focused if it has one); the rest of the double-click is not the
//!   tab's. The middle button never does anything destructive: on a tab it
//!   closes the tab, and closing a worker's tab never touches the worker.
//! - **Keys.** Ctrl+Shift+↑ (Cmd/Option+Shift+↑ on macOS) opens the worker
//!   on screen, else the newest running one, in a tab. The Menu key and
//!   Shift+F10 open the menu of the worker on screen (its tray row), else
//!   of the newest row.
//! - **The menu** (right-click on a tray row, a worker block or a worker
//!   tab): Open, Open in new tab, Steer…, Cancel worker…, Copy handle,
//!   Copy result (once it ended), Copy task. An item that cannot act says
//!   why in its tooltip.
//! - **Cancel.** `Client::cancel_worker`: the engine stops it at its next
//!   step and notes the cancel in the main conversation. Only from a
//!   window that may send the session input (as a steer); a first press
//!   arms it (`Cancel sleeper? CONFIRM`) for [`CONFIRM`], a second one from
//!   [`SETTLE`] on sends it (a double-click's second click and a held
//!   key's repeats are not presses). The worker says `cancelling` until
//!   its rows say how it ended. Also from the × a hovered tray row shows.
//! - **The hover card** on a tray row or a worker block, after the pointer
//!   rests as for any tooltip: its task's first lines, model, elapsed
//!   (once it ended: when it started, how long it ran), tools, its step or
//!   result. Built when shown, from what this window has heard; nothing in
//!   it moves.

use std::time::{Duration, Instant};

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, display, mono};
use crate::fonts;
use crate::fonts::TextPx as _;
use crate::glyph::{self, Ground};
use crate::menu::{At, How, Item, Key, Menu};
use crate::subagents::{Info, Liveness, Subagents, fmt_secs};
use crate::tabs::{Kind, Place};
use crate::theme::{self, Palette};

/// A cancel's first press waits this long for the second.
pub(crate) const CONFIRM: Duration = Duration::from_secs(3);
/// A cancel sent is shown `cancelling` until the rows say so, or this long.
pub(crate) const CANCEL_WAIT: Duration = Duration::from_secs(20);
/// After a click opens a worker's tab, a click this soon with a click
/// count over 1 is the rest of a double-click, not the tab's.
pub(crate) const MULTI_CLICK: Duration = Duration::from_millis(500);

/// The worker key that opens a worker in a tab, as hints spell it.
pub(crate) const TAB_KEY: &str = if cfg!(target_os = "macos") { "⌥⇧↑" } else { "Ctrl+Shift+↑" };

/// Ctrl+Shift+↑ (macOS: Cmd+Shift+↑ outside an input, where it selects to
/// the start, and Option+Shift+↑). Not Alt+Shift+↑ elsewhere: the inputs
/// add a caret above with it.
pub(crate) const fn tab_keys(mac: bool) -> &'static [&'static str] {
    if mac { &["alt-shift-up", "cmd-shift-up"] } else { &["ctrl-shift-up"] }
}

/// A keystroke that opens a context menu from the keyboard.
pub(crate) fn is_menu_key(ks: &Keystroke) -> bool {
    let m = &ks.modifiers;
    match ks.key.as_str() {
        "menu" | "contextmenu" | "apps" => !(m.control || m.alt || m.platform),
        "f10" => m.shift && !(m.control || m.alt || m.platform),
        _ => false,
    }
}

/// Where a click on a worker (its tray row, its block) takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Over the session's thread, in the session's tab.
    InPlace,
    /// In a tab of its own.
    Tab,
    /// Its menu.
    Menu,
}

/// A click with `button`, Ctrl/Cmd held (`secondary`), its `clicks`
/// (2: a double-click); `tab_first`: a left click opens a tab here (a
/// worker tab is on screen, which shows its one worker; or the click is on
/// a block in the transcript, history).
pub(crate) fn route(button: MouseButton, secondary: bool, clicks: usize, tab_first: bool) -> Option<Route> {
    match button {
        MouseButton::Left if secondary || clicks >= 2 || tab_first => Some(Route::Tab),
        MouseButton::Left => Some(Route::InPlace),
        MouseButton::Middle => Some(Route::Tab),
        MouseButton::Right => Some(Route::Menu),
        _ => None,
    }
}

fn button(ev: &ClickEvent) -> MouseButton {
    if ev.is_right_click() {
        MouseButton::Right
    } else if ev.is_middle_click() {
        MouseButton::Middle
    } else {
        MouseButton::Left
    }
}

// ── who ───────────────────────────────────────────────────────────────

/// Whether `a` and `b` (a call's tool id if known, and a name) are the
/// same worker: the same call if both know it, else the same name. `sa_N`
/// alone never says: a cold-started daemon numbers its workers from 1
/// again.
pub(crate) fn same_worker(a: (Option<&str>, &str), b: (Option<&str>, &str)) -> bool {
    match (a.0, b.0) {
        (Some(x), Some(y)) => x == y,
        _ => a.1 == b.1,
    }
}

/// A worker as a tray row, a block, a tab or a menu names it: `sa_N`'s
/// N, its name, and the call that started it once that call's own result
/// named it ([`crate::subagents::Subagents::ident`]). Cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Who {
    pub id: u64,
    pub name: SharedString,
    pub call: Option<SharedString>,
}

impl Who {
    pub fn new(id: u64, name: impl Into<SharedString>, call: Option<SharedString>) -> Self {
        Who { id, name: name.into(), call }
    }

    pub fn handle(&self) -> String {
        format!("sa_{}", self.id)
    }

    /// What [`same_worker`] compares.
    pub fn key(&self) -> (Option<&str>, &str) {
        (self.call.as_deref(), &self.name)
    }

    /// The session's workers say its id names another worker now (`now`:
    /// what [`crate::subagents::Subagents::ident`] says of the id).
    pub fn other(&self, now: Option<(Option<&str>, &str)>) -> bool {
        now.is_some_and(|n| !same_worker(self.key(), n))
    }
}

// ── cancel ────────────────────────────────────────────────────────────

/// A cancel's second press counts from this long after the first: the
/// second click of a double-click, or a held key's first repeat (250 ms
/// at the earliest), never confirms it.
pub(crate) const SETTLE: Duration = Duration::from_millis(400);

/// What a cancel press did ([`Cancels::press`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// The first press: armed for [`CONFIRM`].
    Armed,
    /// A second one before [`SETTLE`]: still armed, nothing sent.
    Early,
    /// The second press: send it.
    Fire,
    /// Not a press: a multi-click's later click, a held key's repeat.
    Ignored,
}

/// A cancel sent, waiting for the daemon's answer and the rows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    session: String,
    handle: String,
    attach_seq: u64,
    at: Instant,
}

/// The worker whose cancel was pressed once, and when: who it is, not
/// only its handle (a cold-started daemon gives `sa_N` to another).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Arm {
    session: String,
    who: Who,
    at: Instant,
}

impl Arm {
    fn is(&self, session: &str, id: u64, key: (Option<&str>, &str), now: Instant) -> bool {
        self.session == session && self.who.id == id && same_worker(self.who.key(), key) && now < self.at + CONFIRM
    }
}

/// The two-step cancel and the cancels on their way. No GPUI.
#[derive(Debug, Default)]
pub(crate) struct Cancels {
    armed: Option<Arm>,
    sent: Vec<Sent>,
    /// Bumped on every change (what the tray says).
    pub generation: u64,
}

impl Cancels {
    /// Cancel pressed for worker `who` of `session`, `how`: the first
    /// press arms it for [`CONFIRM`]; a second one from [`SETTLE`] after
    /// it fires. A held key's repeats are not presses; nor is a
    /// multi-click's later click before [`SETTLE`] (a double-click). From
    /// then on it is: a system whose double-click time is longer (macOS
    /// and Windows let it be seconds) counts a deliberate second click
    /// as one.
    pub fn press(&mut self, session: &str, who: &Who, how: How, now: Instant) -> Press {
        let multi = matches!(how, How::Click(n) if n > 1);
        if !how.is_press() && !multi {
            return Press::Ignored;
        }
        if let Some(a) = self.armed.as_ref().filter(|a| a.is(session, who.id, who.key(), now)) {
            if now.saturating_duration_since(a.at) < SETTLE {
                return if multi { Press::Ignored } else { Press::Early };
            }
            self.armed = None;
            self.generation += 1;
            return Press::Fire;
        }
        // A multi-click never arms: its first click did, or nothing.
        if multi {
            return Press::Ignored;
        }
        self.armed = Some(Arm { session: session.to_string(), who: who.clone(), at: now });
        self.generation += 1;
        Press::Armed
    }

    /// Worker `id` (`key`: [`Who::key`]) of `session` is armed. Per frame
    /// (the tray's rows): nothing allocated.
    pub fn armed(&self, session: &str, id: u64, key: (Option<&str>, &str), now: Instant) -> bool {
        self.armed.as_ref().is_some_and(|a| a.is(session, id, key, now))
    }

    /// When the armed press runs out.
    pub fn armed_until(&self) -> Option<Instant> {
        self.armed.as_ref().map(|a| a.at + CONFIRM)
    }

    /// The armed press ran out: disarmed. Whether that changed anything.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.armed_until().is_some_and(|u| now >= u) {
            return self.disarm();
        }
        false
    }

    /// Anything else was done (another item picked, the menu closed,
    /// another click on a worker): disarmed. Whether it was armed.
    pub fn disarm(&mut self) -> bool {
        if self.armed.take().is_some() {
            self.generation += 1;
            return true;
        }
        false
    }

    pub fn sent(&mut self, session: &str, handle: &str, attach_seq: u64, now: Instant) {
        self.sent.retain(|s| !(s.session == session && s.handle == handle));
        self.sent.push(Sent { session: session.to_string(), handle: handle.to_string(), attach_seq, at: now });
        self.generation += 1;
    }

    /// A cancel of worker `handle` of `session` is on its way.
    pub fn pending(&self, session: &str, handle: &str) -> bool {
        self.sent.iter().any(|s| s.session == session && s.handle == handle)
    }

    /// [`Cancels::pending`] of `sa_{id}`, nothing allocated (per frame).
    pub fn pending_id(&self, session: &str, id: u64) -> bool {
        self.sent.iter().any(|s| s.session == session && s.handle.strip_prefix("sa_").and_then(|n| n.parse().ok()) == Some(id))
    }

    /// The daemon's answer to the cancel of `handle` sent on attachment
    /// `attach_seq`: done, the rows will say it (it stays `cancelling`
    /// until they do); not done, the note to show.
    pub fn answered(&mut self, attach_seq: u64, handle: &str, result: &Result<String, String>) -> Option<String> {
        let i = self.sent.iter().position(|s| s.attach_seq == attach_seq && s.handle == handle)?;
        match result {
            Ok(_) => None,
            Err(why) => {
                self.sent.remove(i);
                self.generation += 1;
                Some(format!("{handle} not cancelled: {why}"))
            }
        }
    }

    /// The cancels of `session`'s workers its rows now say ended or
    /// cancelling (`said`), or that waited [`CANCEL_WAIT`], are done.
    /// Whether any was.
    pub fn settle(&mut self, session: &str, said: impl Fn(&str) -> bool, now: Instant) -> bool {
        let n = self.sent.len();
        self.sent.retain(|s| !(s.session == session && said(&s.handle)) && now.saturating_duration_since(s.at) < CANCEL_WAIT);
        let changed = self.sent.len() != n;
        if changed {
            self.generation += 1;
        }
        changed
    }
}

// ── what a worker is doing ────────────────────────────────────────────

/// The worker a menu is for (`who`, of `session`); `tab`: the worker tab
/// it was opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub session: String,
    pub who: Who,
    pub tab: Option<u64>,
}

/// What a worker's menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Open,
    OpenTab,
    Steer,
    Cancel,
    CopyHandle,
    CopyResult,
    CopyTask,
}

/// What the menu's items depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    pub name: String,
    pub handle: String,
    pub state: Liveness,
    /// Why this window may not steer or cancel it (`None`: it may).
    pub read_only: Option<&'static str>,
    /// Its handle names another worker now (a cold-started daemon).
    pub other: bool,
    /// Its cancel was pressed once ([`Cancels::armed`]) / was sent.
    pub armed: bool,
    pub pending: bool,
    /// Its result as heard: whole (`true`), else a preview of this many
    /// chars.
    pub result: Option<(bool, usize)>,
    pub has_task: bool,
    /// The menu is its worker tab's (no "Open in new tab").
    pub tab: bool,
}

/// A worker's state in the menus' words: §4.2's Mono column.
fn word(l: Liveness) -> &'static str {
    glyph::worker(l).2
}

/// Why a worker's handle no longer names it.
fn other_why(handle: &str, name: &str) -> String {
    format!("{handle} is another worker now (the daemon restarted): {name} is gone")
}

/// The menu of a worker.
pub(crate) fn items(f: &Facts) -> Vec<Item<Act>> {
    let other = f.other.then(|| other_why(&f.handle, &f.name));
    let off = |i: Item<Act>| match &other {
        Some(why) => i.off(why.clone()),
        None => i,
    };
    let mut out = vec![if f.tab { Item::new(Act::Open, "Show") } else { off(Item::new(Act::Open, "Open").key("Click")) }];
    if !f.tab {
        out.push(off(Item::new(Act::OpenTab, "Open in new tab").key("Middle-click")));
    }
    let mut steer = Item::new(Act::Steer, "Steer…");
    if other.is_none() && f.state.ended() {
        steer = steer.off(format!("{} is {}: nothing to steer", f.name, word(f.state)));
    } else if let Some(why) = f.read_only.filter(|_| other.is_none()) {
        steer = steer.off(why);
    }
    out.push(off(steer));
    let cancel = match (f.pending || f.state == Liveness::Cancelling, f.armed) {
        (true, _) => Item::new(Act::Cancel, "cancelling").off(format!("{} stops at its next step", f.name)),
        (false, true) => {
            let mut i = Item::new(Act::Cancel, format!("Cancel {}?", f.name));
            i.confirm = Some(fonts::label::CONFIRM.into());
            i
        }
        (false, false) => Item::new(Act::Cancel, "Cancel worker…"),
    };
    out.push(if other.is_some() {
        off(cancel)
    } else if f.state.ended() && !f.pending {
        cancel.off(format!("{} is {}: nothing to cancel", f.name, word(f.state)))
    } else if let Some(why) = f.read_only {
        cancel.off(why)
    } else {
        cancel
    });
    out.push(Item::new(Act::CopyHandle, format!("Copy handle ({})", f.handle)));
    let mut result = match f.result {
        Some((false, n)) => Item::new(Act::CopyResult, format!("Copy result preview (first {n} chars)")),
        _ => Item::new(Act::CopyResult, "Copy result"),
    };
    if other.is_none() && (!f.state.ended() || f.state == Liveness::Gone) {
        result = result.off("once it has finished");
    } else if other.is_none() && f.result.is_none() {
        result = result.off("no result heard: collect it (subagent_collect) or open its thread");
    }
    out.push(off(result));
    let mut task = Item::new(Act::CopyTask, "Copy task");
    if other.is_none() && !f.has_task {
        task = task.off("its task is not in this transcript");
    }
    out.push(off(task));
    out
}

/// Why a cancel may not go ([`AppView::cancel_blocked`]), said only when
/// it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blocked {
    Drive(&'static str),
    /// The session's workers know nothing of the handle.
    Unknown,
    /// The handle names another worker now.
    Other,
    /// The same call, under another name now than the menu or the row
    /// had (it was opened before the call's result named the worker).
    Renamed,
    Ended(Liveness),
}

impl Blocked {
    fn say(self, who: &Who) -> String {
        match self {
            Blocked::Drive(why) => format!("{} not cancelled: {why}", who.handle()),
            Blocked::Unknown => format!("{} not cancelled: this window knows no worker by that handle", who.handle()),
            Blocked::Other => format!("{}: nothing sent", other_why(&who.handle(), &who.name)),
            Blocked::Renamed => format!("{} not cancelled: {} goes by another name now; open its menu again", who.handle(), who.name),
            Blocked::Ended(s) => format!("{} is {}: nothing to cancel", who.name, word(s)),
        }
    }
}

impl AppView {
    /// The session whose workers the tray and the blocks show.
    pub(crate) fn worker_session(&self) -> Option<String> {
        self.model.lives[self.panes.cur.live]
            .active
            .as_ref()
            .map(|a| a.session_id.clone())
            .or_else(|| self.model.tabs.current_session().map(str::to_string))
    }

    /// What the session calls itself (a worker tab's title, its parent).
    pub(crate) fn session_label(&self, session: &str) -> String {
        match self.model.sessions.iter().find(|m| m.id == session) {
            Some(m) => m.label(),
            None => self
                .model
                .tabs
                .iter()
                .find(|t| t.session.as_deref() == Some(session))
                .map_or_else(|| session.to_string(), |t| t.label.clone()),
        }
    }

    /// The workers of `session` as this window knows them (attached, or in
    /// the tab cache).
    fn subs_of(&self, session: &str) -> Option<&Subagents> {
        if self.model.is_active(session) {
            return Some(&self.model.lives[self.panes.cur.live].stats.subagents);
        }
        self.tab_cache.peek(session).map(|c| &c.stats.subagents)
    }

    /// What this window knows of `who` of `session`, if its handle still
    /// names it.
    fn info_of(&self, session: &str, who: &Who) -> Option<Info> {
        let subs = self.subs_of(session)?;
        if who.other(subs.ident(who.id)) {
            return None;
        }
        subs.info(who.id)
    }

    /// The worker tab on screen holds a worker its handle no longer names
    /// (the attached session's workers say another one has it now).
    pub(crate) fn tab_worker_reused(&self) -> bool {
        let Some(t) = self.model.tabs.current().filter(|t| t.kind == Kind::Worker) else { return false };
        let (Some(s), Some(w)) = (t.session.as_deref(), t.worker.as_ref()) else { return false };
        let id = w.id().filter(|_| self.model.is_active(s));
        id.and_then(|id| self.model.lives[self.panes.cur.live].stats.subagents.ident(id)).is_some_and(|now| !same_worker(w.who(), now))
    }

    /// The worker tab on screen holds a worker its handle no longer names:
    /// its view is gone (no polls, no steer), saying who has the handle.
    pub(crate) fn disown_reused(&mut self) {
        if !self.tab_worker_reused() {
            return;
        }
        let Some(w) = self.model.tabs.current().and_then(|t| t.worker.clone()) else { return };
        let now = w
            .id()
            .and_then(|id| self.model.lives[self.panes.cur.live].stats.subagents.ident(id))
            .map_or_else(String::new, |(_, n)| format!(" ({n})"));
        let why = || format!("{} is another worker now{now} (the daemon restarted): {} is gone", w.handle, w.name);
        if self.model.lives[self.panes.cur.live].watch.disown(&w.handle, why) {
            self.model.view_dirty = true;
        }
    }

    /// The NEEDS YOU stub's click: the session of the worker on screen
    /// waits on a prompt. Its tab (opened if closed) with its thread on
    /// screen, and so its card: never answered from a worker's view.
    pub(crate) fn open_parent_card(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.model.tabs.current().filter(|t| t.kind == Kind::Worker)
            && let Some(ix) = t.session.as_deref().and_then(|s| self.model.tabs.find(s))
        {
            let id = self.model.tabs.iter().nth(ix).map(|t| t.id);
            if let Some(id) = id {
                self.model.tabs.set_watching(id, None);
            }
        }
        self.leave_worker(false, cx);
    }

    /// The worker the attached session's workers know as `sa_{id}` now.
    fn who_now(&self, id: u64) -> Option<Who> {
        let (call, name) = self.model.lives[self.panes.cur.live].stats.subagents.ident(id)?;
        Some(Who::new(id, name.to_string(), call.map(|c| c.to_string().into())))
    }

    /// Worker `handle` (`sa_N`) of `session`, as every surface shows it
    /// ([`Liveness`], design.md §4.3): from its session's workers (attached
    /// here, or as its tab last saw them), which its view's pages feed.
    /// A cancel on its way is `Cancelling`;
    /// `expect` (who a tab, a menu or the foot says it is): another worker
    /// under the handle now is `Gone`, as is one an attached session does
    /// not know. `None`: this window holds nothing of its session (not
    /// attached, not cached). Read per frame for a worker tab's mark and
    /// the composer foot: nothing allocated.
    pub(crate) fn worker_state(&self, session: &str, handle: &str, expect: Option<(Option<&str>, &str)>) -> Option<Liveness> {
        let attached = self.model.is_active(session);
        let shown = attached || self.panes.cur.shown_tab.as_ref().and_then(|t| t.1.as_deref()) == Some(session);
        let subs = match (shown, self.tab_cache.peek(session)) {
            (true, _) => &self.model.lives[self.panes.cur.live].stats.subagents,
            (false, Some(c)) => &c.stats.subagents,
            (false, None) => return None,
        };
        let id = handle.strip_prefix("sa_").and_then(|n| n.parse().ok());
        let now = id.and_then(|id| subs.ident(id));
        if let (Some(e), Some(now)) = (expect, now)
            && !same_worker(e, now)
        {
            return Some(Liveness::Gone);
        }
        // One source: the session's list (its views' pages feed it,
        // [`crate::model::Model::heard_view`]).
        let status = id.and_then(|id| subs.status(id));
        let live = Liveness::of(status, attached, !attached || now.is_some() || status.is_some());
        Some(live.cancelling(self.model.lives[self.panes.cur.live].cancels.pending(session, handle)))
    }

    /// Whether this window may steer or cancel `session`'s workers, else
    /// why not (the reason that applies).
    pub(crate) fn may_drive(&self, session: &str) -> Result<(), &'static str> {
        if self.client.is_none() {
            return Err("offline: no daemon to send to");
        }
        if !self.model.is_active(session) {
            return Err("its session is not attached in this window: show its tab first");
        }
        self.model.lives[self.panes.cur.live].input_gate()
    }

    /// Why a cancel of `who` (`handle`) of `session` may not go now, if it
    /// may not: this window may not drive it, the handle names another
    /// worker (or none this window knows), or it has ended. Per frame for
    /// the tray's ×: nothing allocated.
    fn cancel_blocked(&self, session: &str, who: &Who, handle: &str) -> Option<Blocked> {
        if let Err(why) = self.may_drive(session) {
            return Some(Blocked::Drive(why));
        }
        // The session is attached (`may_drive`): its workers are the model's.
        let Some(now) = self.model.lives[self.panes.cur.live].stats.subagents.ident(who.id) else { return Some(Blocked::Unknown) };
        if !same_worker(who.key(), now) {
            return Some(Blocked::Other);
        }
        // Its name too, whatever the call says (N1: the menu's `other` goes
        // by the call, so a renamed worker is said as one).
        if now.1 != who.name.as_ref() {
            return Some(Blocked::Renamed);
        }
        match self.worker_state(session, handle, Some(who.key())) {
            None | Some(Liveness::Running | Liveness::Started) => None,
            Some(s) => Some(Blocked::Ended(s)),
        }
    }

    /// A click on a worker (its tray row; `block`: its block in the
    /// transcript, history: a left click opens a tab), as [`route`] says.
    pub(crate) fn worker_click(&mut self, who: &Who, block: bool, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let secondary = crate::tabstrip::click_place(ev, cx) == Place::New;
        let tab_first = block || self.model.tabs.current_worker().is_some();
        match route(button(ev), secondary, ev.click_count(), tab_first) {
            Some(Route::InPlace) => {
                self.close_menu();
                self.watch_worker(who, cx);
            }
            Some(Route::Tab) => {
                self.close_menu();
                // The rest of a double-click is not for the tab it opens.
                self.swallow_clicks = Some(Instant::now() + MULTI_CLICK);
                self.open_worker_tab(who, cx);
            }
            Some(Route::Menu) => {
                let Some(session) = self.worker_session() else { return };
                let t = Target { session, who: who.clone(), tab: None };
                self.open_menu(t, At::Point(ev.position()), false, window, cx);
            }
            None => {}
        }
    }

    /// Open `who` of the session on screen in its own tab (focused if it
    /// has one). Its view over the session's thread, if it was there,
    /// gives the thread back.
    pub(crate) fn open_worker_tab(&mut self, who: &Who, cx: &mut Context<Self>) {
        let Some(session) = self.worker_session() else { return };
        let handle = who.handle();
        if self.model.lives[self.panes.cur.live].watch.focus() == Some(handle.as_str())
            && !self.model.lives[self.panes.cur.live].watch.in_tab()
        {
            self.model.lives[self.panes.cur.live].watch.focus_main();
        }
        let label = self.session_label(&session);
        let had = self.model.tabs.iter().map(|t| t.id).collect::<Vec<_>>();
        if self.model.tabs.open_worker_as(&session, &handle, &who.name, who.call.as_deref(), &label) {
            self.model.rail_gen += 1;
            self.reconcile(None);
            // The tracer only to a tab this opened (focusing one is not).
            self.panes.cur.tracer_open = self.model.tabs.current().map(|t| t.id).filter(|id| !had.contains(id));
        }
        cx.notify();
    }

    /// Ctrl+Shift+↑: the worker on screen, else the newest running one, in
    /// a tab.
    pub(crate) fn worker_tab_key(&mut self, cx: &mut Context<Self>) {
        if self.model.tabs.view().is_some() || self.model.tabs.current_worker().is_some() {
            return;
        }
        let pick = match self.model.lives[self.panes.cur.live].watch.focused() {
            Some(w) => self.who_now(w.subagent_id).or_else(|| Some(Who::new(w.subagent_id, w.name.clone(), None))),
            None if self.model.lives[self.panes.cur.live].active.is_some() => {
                self.model.lives[self.panes.cur.live].watchable().last().and_then(|c| self.who_now(c.subagent_id))
            }
            None => None,
        };
        match pick {
            Some(who) => self.open_worker_tab(&who, cx),
            None => {
                self.panes.cur.watch_note = Some("no running workers to open in a tab".into());
                cx.notify();
            }
        }
    }

    /// The tab strip's right-click on worker tab `tab_id`.
    pub(crate) fn worker_tab_menu(&mut self, tab_id: u64, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.model.tabs.iter().find(|t| t.id == tab_id && t.kind == Kind::Worker) else { return };
        let (Some(session), Some(w)) = (t.session.clone(), t.worker.as_ref()) else { return };
        let Some(id) = w.id() else { return };
        let who = Who::new(id, w.name.clone(), w.call.clone().map(Into::into));
        self.open_menu(Target { session, who, tab: Some(tab_id) }, At::Point(at), false, window, cx);
    }

    /// The Menu key / Shift+F10: the menu of the worker on screen (its
    /// tray row), else of the newest row.
    pub(crate) fn menu_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.tray_drawn();
        let rows = &self.tray.rows[..shown];
        let watched = self.model.lives[self.panes.cur.live].watch.focused().map(|w| w.subagent_id);
        let row = watched.and_then(|id| rows.iter().find(|r| r.id == id)).or_else(|| rows.iter().rev().find(|r| r.watchable));
        let (Some(r), Some(session)) = (row.filter(|r| r.watchable), self.worker_session()) else {
            self.panes.cur.watch_note = Some("no worker here: the tray over the composer lists the session's workers".into());
            cx.notify();
            return;
        };
        let tab = self.model.tabs.current().filter(|t| t.worker.as_ref().and_then(|w| w.id()) == Some(r.id)).map(|t| t.id);
        let t = Target { session, who: Who::new(r.id, r.name.clone(), r.call.clone()), tab };
        let at = At::Row(r.id);
        self.open_menu(t, at, true, window, cx);
    }

    fn facts(&self, t: &Target, now: Instant) -> Facts {
        let handle = t.who.handle();
        let subs = self.subs_of(&t.session);
        let other = t.who.other(subs.and_then(|s| s.ident(t.who.id)));
        let info = if other { None } else { subs.and_then(|s| s.info(t.who.id)) };
        Facts {
            name: t.who.name.to_string(),
            // Nothing held of its session: not ended, as far as this window knows.
            state: if other {
                Liveness::Gone
            } else {
                self.worker_state(&t.session, &handle, Some(t.who.key())).unwrap_or(Liveness::Started)
            },
            read_only: self.may_drive(&t.session).err(),
            other,
            armed: self.model.lives[self.panes.cur.live].cancels.armed(&t.session, t.who.id, t.who.key(), now),
            pending: self.model.lives[self.panes.cur.live].cancels.pending(&t.session, &handle),
            result: info.as_ref().filter(|i| !i.result.is_empty()).map(|i| (i.whole, i.result.chars().count())),
            has_task: info.as_ref().is_some_and(|i| !i.task_full.is_empty()),
            tab: t.tab.is_some(),
            handle,
        }
    }

    /// Open the menu of `t`. The view takes the focus while it is open,
    /// so Esc and the arrows reach the menu, not an input's own bindings
    /// (the composer's); what had it gets it back once the menu closes.
    pub(crate) fn open_menu(&mut self, t: Target, at: At, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = window.focused(cx).filter(|f| *f != self.focus)
            && self.menu_focus_back.is_none()
        {
            self.menu_focus_back = Some(prev);
        }
        window.focus(&self.focus, cx);
        self.gear_menu = false;
        // A cancel armed for another worker, or from the tray, waits no more.
        if !self.menu.as_ref().is_some_and(|(_, old)| old.who == t.who) {
            self.model.lives[self.panes.cur.live].cancels.disarm();
        }
        let items = items(&self.facts(&t, Instant::now()));
        let m = Menu::new(items, at, keyboard);
        // A pointer's menu enters; the keyboard's is there at once.
        let m = if keyboard { m } else { m.born(self.fx.clock()) };
        self.menu = Some((m, t));
        cx.notify();
    }

    /// The menu closes; anything else done means an armed cancel no longer
    /// waits for its second press.
    pub(crate) fn close_menu(&mut self) {
        self.menu = None;
        self.smenu = None;
        self.wmenu = None;
        if self.model.lives[self.panes.cur.live].cancels.disarm() {
            self.cancel_tick = None;
        }
    }

    /// The open menu's items again (a cancel armed or run out, the tray's
    /// workers changed), the selection kept.
    pub(crate) fn refresh_menu(&mut self) {
        let Some((m, t)) = self.menu.as_ref() else { return };
        let items = items(&self.facts(t, Instant::now()));
        if items == m.items {
            return;
        }
        let (at, sel, born) = (m.at, m.sel, m.born);
        if let Some((m, _)) = self.menu.as_mut() {
            *m = Menu { items, sel, at, born };
        }
    }

    /// A key while the menu is open (`press`: a new press, not a held
    /// key's repeat: [`crate::app::CardChord`]): `true` if the menu took it.
    pub(crate) fn menu_keydown(&mut self, ks: &Keystroke, press: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((m, _)) = self.menu.as_mut() else { return false };
        match m.key(&ks.key, ks.modifiers.shift, press) {
            Key::Moved => {}
            Key::Pick(a) => {
                // Its repeats, once the menu is gone, are not the next
                // thing's (a held Enter on Steer… would send the draft).
                self.menu_picked = true;
                self.menu_pick(a, How::Key { press }, window, cx);
            }
            Key::Close => self.close_menu(),
            Key::Other => {
                self.close_menu();
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The Enter an input just emitted (`PressEnter`) is a held repeat of
    /// the key that picked a menu item: it does nothing. The input's Enter
    /// binding ran before the root's key listener could stop it; the event
    /// arrives after that listener ([`AppView::menu_picked`]).
    pub(crate) fn enter_is_the_picks(&self) -> bool {
        self.menu_picked
    }

    /// An item picked, `how`.
    pub(crate) fn menu_pick(&mut self, act: Act, how: How, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, t)) = self.menu.clone() else { return };
        if act == Act::Cancel {
            self.press_cancel(&t.session, &t.who, how, cx);
            return;
        }
        if !how.is_press() {
            return;
        }
        self.close_menu();
        // Its handle names another worker now: nothing of that one's.
        if act != Act::CopyHandle && !(act == Act::Open && t.tab.is_some()) && self.facts(&t, Instant::now()).other {
            self.panes.cur.watch_note = Some(other_why(&t.who.handle(), &t.who.name).into());
            cx.notify();
            return;
        }
        let copy = |text: String, cx: &mut Context<Self>| cx.write_to_clipboard(ClipboardItem::new_string(text));
        match act {
            Act::Open | Act::Steer => {
                match t.tab {
                    Some(tab) => {
                        let ix = self.model.tabs.iter().position(|x| x.id == tab);
                        if let Some(ix) = ix {
                            self.focus_tab(ix);
                        }
                    }
                    None if self.model.tabs.current_worker().is_some() => self.open_worker_tab(&t.who, cx),
                    None => self.watch_worker(&t.who, cx),
                }
                if act == Act::Steer {
                    self.panes.cur.composer.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            Act::OpenTab => self.open_worker_tab(&t.who, cx),
            Act::CopyHandle => copy(t.who.handle(), cx),
            Act::CopyResult => {
                if let Some(i) = self.info_of(&t.session, &t.who) {
                    copy(i.result, cx);
                }
            }
            Act::CopyTask => {
                if let Some(i) = self.info_of(&t.session, &t.who) {
                    copy(i.task_full, cx);
                }
            }
            Act::Cancel => {}
        }
        cx.notify();
    }

    /// Cancel pressed (`how`: the menu's item, a tray row's ×) for `who`
    /// of `session`: armed, or (the second press, [`SETTLE`] after the
    /// first) sent. One that may not go closes the menu and says why.
    pub(crate) fn press_cancel(&mut self, session: &str, who: &Who, how: How, cx: &mut Context<Self>) {
        let now = Instant::now();
        let handle = who.handle();
        if let Some(b) = self.cancel_blocked(session, who, &handle) {
            if how.is_press() {
                self.close_menu();
                self.panes.cur.watch_note = Some(b.say(who).into());
                cx.notify();
            }
            return;
        }
        let pressed = self.model.lives[self.panes.cur.live].cancels.press(session, who, how, now);
        log::debug!(target: "synaps_desktop::watch", "cancel press {handle}: {how:?} -> {pressed:?}");
        match pressed {
            Press::Fire => {
                let seq = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.attach_seq);
                match (&self.client, seq) {
                    (Some(c), Some(seq)) if c.on(seq).cancel_worker(handle.clone()).is_ok() => {
                        log::debug!(target: "synaps_desktop::watch", "cancel {handle} ({})", who.name);
                        self.model.lives[self.panes.cur.live].cancels.sent(session, &handle, seq, now);
                    }
                    _ => self.panes.cur.watch_note = Some(format!("{handle} not cancelled: the connection thread has stopped").into()),
                }
                self.close_menu();
                self.cancel_tick = None;
            }
            Press::Armed => {
                self.arm_cancel_tick(cx);
                self.refresh_menu();
            }
            Press::Early | Press::Ignored => return,
        }
        cx.notify();
    }

    /// The armed cancel's end draws (once, when it runs out).
    fn arm_cancel_tick(&mut self, cx: &mut Context<Self>) {
        let Some(until) = self.model.lives[self.panes.cur.live].cancels.armed_until() else { return };
        self.cancel_tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(until.saturating_duration_since(Instant::now())).await;
            let _ = this.update(cx, |v, cx| {
                v.cancel_tick = None;
                if v.model.lives[v.panes.cur.live].cancels.expire(Instant::now()) {
                    v.refresh_menu();
                    cx.notify();
                }
            });
        }));
    }

    /// The menu, if open at a point (a right-click): at the root, over
    /// everything.
    /// One leaving (fluid.md §3.10) is drawn without a hitbox until gone.
    pub(crate) fn render_menu_at_point(&self, p: &Palette, cx: &Context<Self>) -> Option<impl IntoElement> {
        let (m, phase) = match self.menu.as_ref() {
            Some((m, _)) => (m, crate::menu::Phase::enter(&self.fx, m.born, -1.)),
            None => {
                let (m, at) = self.menu_leaving.as_ref()?;
                (m, crate::menu::Phase::leave(&self.fx, *at, -1.)?)
            }
        };
        let At::Point(pt) = m.at else { return None };
        let body = crate::menu::render("worker-menu", m, p, AppView::menu_pick, |this, _| this.close_menu(), phase, &self.fx, cx);
        Some(deferred(anchored().position(pt).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
    }

    /// The menu, if opened from the keyboard over tray row `id`: drawn by
    /// that row, above it.
    pub(crate) fn render_menu_at_row(&self, id: u64, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let (m, _) = self.menu.as_ref()?;
        if m.at != At::Row(id) {
            return None;
        }
        let phase = crate::menu::Phase::enter(&self.fx, m.born, 1.);
        let body = crate::menu::render("worker-menu", m, p, AppView::menu_pick, |this, _| this.close_menu(), phase, &self.fx, cx);
        Some(
            div()
                .absolute()
                .left(px(24.))
                .top_0()
                .child(deferred(anchored().anchor(Anchor::BottomLeft).snap_to_window_with_margin(px(8.)).child(body)).with_priority(3))
                .into_any_element(),
        )
    }

    /// The fuse before an armed cancel's CONFIRM (motion.md §6.2): three
    /// 4 px cells in the err family's mark on the tray's ground, which is
    /// `err` itself there (design.md §4.4's table: `err_mark_ch` is the
    /// chrome's, `err_mark_k` the card's), one going dark per second of [`CONFIRM`] on
    /// the clock; Reduced, all three until it runs out; Off, none.
    fn fuse(&self, p: &Palette) -> Option<Div> {
        let until = self.model.lives[self.panes.cur.live].cancels.armed_until()?;
        let lit = match self.fx.level() {
            crate::motion::Level::Off => return None,
            crate::motion::Level::Reduced => 3,
            crate::motion::Level::Full => {
                let left = until.saturating_duration_since(self.fx.now());
                let n = left.as_millis().div_ceil(1000).min(3) as u32;
                if n > 0 {
                    self.fx.tick_at(until - Duration::from_secs(u64::from(n - 1)));
                }
                n
            }
        };
        Some(
            h_flex()
                .debug_selector(|| "tray-fuse".into())
                .flex_none()
                .gap(px(4.))
                .children((0..3).map(|i| div().size(px(4.)).when(i < lit, |d| d.bg(p.err)))),
        )
    }

    /// A tray row's × (on hover, or while its row is the watched one;
    /// armed: `Cancel sleeper? CONFIRM`), if this window may cancel it: the
    /// row's full height `h`, so a near miss is not a click on the row. A
    /// double-click's second click is not a second press
    /// ([`Cancels::press`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cancel_x(
        &self,
        who: &Who,
        handle: &str,
        armed: bool,
        watched: bool,
        h: f32,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.model.lives[self.panes.cur.live].active.as_ref()?.session_id.as_str();
        if self.cancel_blocked(session, who, handle).is_some() {
            return None;
        }
        let el = if armed {
            // The name as its chip cuts it (what its width counts).
            let name = crate::tray::cut(&who.name, crate::tray::NAME_CHARS);
            // The chip's rest (its width is the grid's): `Cancel name?`
            // gives way, cut `…`; the fuse and CONFIRM end the chip, where
            // the × was.
            h_flex()
                .id(("tray-x", who.id as usize))
                .debug_selector(|| "tray-armed".into())
                .flex_1()
                .min_w_0()
                .h(px(h))
                .gap(px(6.))
                .items_center()
                .cursor_pointer()
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(p.err).child(format!("Cancel {name}?")))
                // Before CONFIRM, 8 px from it: the pointer that armed it
                // rests on the ×'s end, where CONFIRM is (finish gate fix 2).
                .children(self.fuse(p).map(|f| f.mr(px(2.))))
                .child(display(fonts::label::CONFIRM, crate::fonts::size::D_S, p.err).debug_selector(|| "tray-confirm".into()))
        } else {
            // A reveal button (design.md §5): drawn while its row is hovered.
            h_flex()
                .id(("tray-x", who.id as usize))
                .group("tray-x")
                .flex_none()
                .w(px(16.))
                .h(px(h))
                .justify_center()
                .cursor_pointer()
                .child(crate::glyph::reveal(crate::glyph::Glyph::Close, "tray-row", "tray-x", watched, p.dim_t, p.ink))
        };
        let tip = if armed { "click again to cancel it (it stops at its next step)" } else { "cancel this worker…" };
        let who = who.clone();
        Some(
            el.tooltip(move |window, cx| crate::overlay::tip(tip, window, cx))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    // The tray lists the attached session's workers.
                    let Some(session) = this.model.lives[this.panes.cur.live].active.as_ref().map(|a| a.session_id.clone()) else { return };
                    this.press_cancel(&session, &who, How::Click(ev.click_count()), cx);
                }))
                .on_aux_click(|_, _, cx| cx.stop_propagation())
                .into_any_element(),
        )
    }
}

/// The hover card of `who` of the session on screen (a tray row, a worker
/// block): built when it shows, from what this window has heard of it
/// (nothing of another worker under its handle now); `fallback` if
/// nothing.
pub(crate) fn card_tip(me: WeakEntity<AppView>, who: Who, fallback: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let info = me.upgrade().and_then(|v| {
            let v = v.read(cx);
            let session = v.worker_session()?;
            let info = v.info_of(&session, &who)?;
            Some((info, v.worker_state(&session, &who.handle(), Some(who.key()))?))
        });
        let Some((info, state)) = info else { return crate::overlay::tip(fallback.clone(), window, cx) };
        let p = theme::palette(cx);
        // Running, ELAPSED says when it started.
        let started = info
            .started
            .filter(|_| state.ended())
            .map(|t| format!("{} ago", fmt_secs(Instant::now().saturating_duration_since(t).as_secs_f64())));
        crate::overlay::element(move |_, _| card(&info, state, started.as_deref(), &p), window, cx)
    }
}

/// The hover card's outer width (design.md §8.7: the overlay's 1-px frame
/// inside it), and its padding.
const CARD_W: f32 = 340.;
const CARD_PAD: f32 = 16.;

/// The card's body (design.md §8.7, on the overlay box): the worker's
/// glyph in the lead column, its name and handle, its word (§4.2's Display
/// column) at the trail; its task, facts and step under them, at the
/// title's edge. 16 all round, 8 between parts.
fn card(i: &Info, state: Liveness, started: Option<&str>, p: &Palette) -> Div {
    use crate::fonts::size;
    use crate::overlay::Fact;
    let (g, word, _, family) = glyph::worker(state);
    let (color, mark) = glyph::ink(family, Ground::Card, p);
    let mut facts = Vec::with_capacity(4);
    if !i.model.is_empty() {
        facts.push(Fact::new(fonts::label::R_MODEL, i.model.clone()));
    }
    if let Some(s) = started {
        facts.push(Fact::new(fonts::label::STARTED, s.to_string()));
    }
    if let Some(e) = i.elapsed {
        facts.push(Fact::new(if state.ended() { fonts::label::RAN } else { fonts::label::ELAPSED }, fmt_secs(e)));
    }
    if let Some(n) = i.tools {
        facts.push(Fact::new(fonts::label::TOOLS, n.to_string()));
    }
    // The lead column: the 10 px glyph centred in 12, then 8.
    let under = || v_flex().pl(px(20.)).min_w_0();
    v_flex()
        .w(px(CARD_W - 2.))
        .p(px(CARD_PAD))
        .gap(px(8.))
        .child(
            h_flex()
                .h(px(18.))
                .items_center()
                .child(div().w(px(12.)).flex_none().flex().justify_center().child(glyph::draw_on(g, mark, p.card)))
                .child(
                    h_flex()
                        .ml(px(8.))
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .items_baseline()
                        .child(div().flex_none().text_px(size::TITLE).text_color(p.ink).child(i.name.clone()))
                        .child(mono(format!("sa_{}", i.id), size::META, p.dim_k).flex_none()),
                )
                .child(display(word, size::D_S, color).flex_none().ml(px(8.))),
        )
        .when(!i.task.is_empty(), |d| {
            d.child(
                under().child(
                    div()
                        .font_family(fonts::MONO)
                        .text_px(size::META)
                        .line_height(px(18.))
                        .max_h(px(72.))
                        .overflow_hidden()
                        .text_color(p.prose_k)
                        .child(i.task.clone()),
                ),
            )
        })
        .when(!facts.is_empty(), |d| d.child(under().child(crate::overlay::facts(facts, p))))
        .when(!i.line.is_empty(), |d| {
            d.child(
                under().child(
                    div()
                        .text_px(size::TITLE)
                        .line_height(px(18.))
                        .text_color(if state == Liveness::Failed { p.err_k } else { p.dim_k })
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(i.line.clone()),
                ),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui_kit::{Keystroke, MouseButton};
    use serde_json::json;

    use super::{Act, CANCEL_WAIT, CONFIRM, Cancels, Facts, Press, Route, SETTLE, Who, is_menu_key, items, route, same_worker};
    use crate::menu::How;
    use crate::subagents::Liveness;
    use crate::subagents::Subagents;

    #[test]
    fn clicks_on_a_worker_go_where_the_button_says() {
        use MouseButton::*;
        // A left click: in place; with Ctrl/Cmd or twice, in a worker tab,
        // or on a block in the transcript (history): a tab of its own.
        assert_eq!(route(Left, false, 1, false), Some(Route::InPlace));
        assert_eq!(route(Left, true, 1, false), Some(Route::Tab));
        assert_eq!(route(Left, false, 2, false), Some(Route::Tab));
        assert_eq!(route(Left, false, 1, true), Some(Route::Tab));
        // The middle button: a tab, never anything destructive; the right one: the menu.
        assert_eq!(route(Middle, false, 1, false), Some(Route::Tab));
        assert_eq!(route(Middle, true, 1, true), Some(Route::Tab));
        assert_eq!(route(Right, false, 1, false), Some(Route::Menu));
        assert_eq!(route(Navigate(gpui_kit::NavigationDirection::Back), false, 1, false), None);
    }

    #[test]
    fn the_menu_key_and_shift_f10_open_the_menu() {
        fn k(s: &'static str) -> Keystroke {
            Keystroke::parse(s).unwrap()
        }
        assert!(is_menu_key(&k("menu")) && is_menu_key(&k("shift-f10")));
        assert!(!is_menu_key(&k("f10")) && !is_menu_key(&k("ctrl-shift-f10")) && !is_menu_key(&k("ctrl-menu")));
    }

    fn facts() -> Facts {
        Facts {
            name: "sleeper".into(),
            handle: "sa_2".into(),
            state: Liveness::Running,
            read_only: None,
            other: false,
            armed: false,
            pending: false,
            result: None,
            has_task: true,
            tab: false,
        }
    }

    const CLICK: How = How::Click(1);
    const W2: (Option<&str>, &str) = (Some("t_2"), "sleeper");
    const W3: (Option<&str>, &str) = (Some("t_3"), "scout");

    fn w2() -> Who {
        Who::new(2, W2.1, W2.0.map(Into::into))
    }

    fn w3() -> Who {
        Who::new(3, W3.1, W3.0.map(Into::into))
    }
    const KEY: How = How::Key { press: true };

    #[test]
    fn cancel_needs_a_second_press_within_the_window() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut c = Cancels::default();
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed, "the first press arms");
        assert!(c.armed("s", 2, W2, ms(100)));
        assert!(!c.armed("s", 3, W3, t0) && !c.armed("t", 2, W2, t0), "only that worker of that session");
        assert_eq!(c.press("s", &w2(), CLICK, t0 + CONFIRM - Duration::from_millis(1)), Press::Fire, "the second within the window fires");
        assert!(!c.armed("s", 2, W2, t0), "and disarms");
        // Too late: the second press arms again instead.
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed);
        assert_eq!(c.press("s", &w2(), CLICK, t0 + CONFIRM), Press::Armed, "out of time: armed again, not fired");
        assert_eq!(c.press("s", &w2(), KEY, t0 + CONFIRM + Duration::from_secs(1)), Press::Fire);
        // Another worker's press re-arms for that one.
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed);
        assert_eq!(c.press("s", &w3(), CLICK, ms(10)), Press::Armed);
        assert_eq!(c.press("s", &w2(), CLICK, ms(20)), Press::Armed, "sa_2's arm went to sa_3");
        // It runs out.
        assert_eq!(c.armed_until(), Some(ms(20) + CONFIRM));
        assert!(!c.expire(ms(30)));
        assert!(c.expire(ms(20) + CONFIRM));
        assert_eq!(c.armed_until(), None);
        // Anything else done disarms it.
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed);
        assert!(c.disarm() && !c.disarm());
        assert_eq!(c.press("s", &w2(), CLICK, ms(500)), Press::Armed, "armed again, not fired");
    }

    /// R2-L2: an arm is the worker's, not its handle's. After a cold
    /// restart, `builder` under `sleeper`'s sa_2 is not armed, and its
    /// first press arms it; a call learned since the press is still the
    /// same worker (by name).
    #[test]
    fn an_arm_is_the_workers_not_its_handles() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let builder = Who::new(2, "builder", Some("t_9".into()));
        let mut c = Cancels::default();
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed);
        assert!(!c.armed("s", 2, builder.key(), ms(10)), "not builder's row");
        assert!(!c.armed("s", 2, (None, "builder"), ms(10)));
        assert_eq!(c.press("s", &builder, CLICK, ms(500)), Press::Armed, "builder's first press");
        assert!(!c.armed("s", 2, W2, ms(510)));
        let unnamed = Who::new(2, "builder", None);
        assert!(c.armed("s", 2, unnamed.key(), ms(520)));
        assert_eq!(c.press("s", &unnamed, CLICK, ms(1000)), Press::Fire);
    }

    /// H1: a double-click, a held key or a second press right after the
    /// first never confirms a cancel: only a press from [`SETTLE`] on.
    #[test]
    fn a_double_click_or_a_held_key_never_confirms_a_cancel() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        // Two presses 50 ms apart: armed, then too early (still armed).
        let mut c = Cancels::default();
        assert_eq!(c.press("s", &w2(), CLICK, t0), Press::Armed);
        assert_eq!(c.press("s", &w2(), CLICK, ms(50)), Press::Early);
        assert!(c.armed("s", 2, W2, ms(60)), "still armed, from the first press");
        assert_eq!(c.press("s", &w2(), CLICK, ms(500)), Press::Fire);
        // A double-click: its second click (click count 2) is no press
        // before SETTLE; the third neither.
        let mut c = Cancels::default();
        assert_eq!(c.press("s", &w2(), How::Click(1), t0), Press::Armed);
        assert_eq!(c.press("s", &w2(), How::Click(2), ms(80)), Press::Ignored);
        assert_eq!(c.press("s", &w2(), How::Click(3), ms(390)), Press::Ignored);
        assert!(c.armed("s", 2, W2, ms(700)));
        // R2-L3: a long system double-click time (macOS, Windows) makes a
        // deliberate second click a count of 2: from SETTLE on, it counts.
        assert_eq!(c.press("s", &w2(), How::Click(2), ms(400)), Press::Fire);
        // A multi-click never arms.
        assert_eq!(c.press("s", &w2(), How::Click(2), ms(500)), Press::Ignored);
        assert!(!c.armed("s", 2, W2, ms(510)));
        // A held Enter (or Space): the press arms, the repeats (from 250 ms
        // on every ~30 ms, for seconds) never fire.
        let mut c = Cancels::default();
        assert_eq!(c.press("s", &w2(), KEY, t0), Press::Armed);
        for n in (250..2900).step_by(33) {
            assert_eq!(c.press("s", &w2(), How::Key { press: false }, ms(n)), Press::Ignored, "repeat at {n} ms");
        }
        assert!(c.armed("s", 2, W2, ms(2900)));
        // Let go and pressed again: that one confirms.
        assert_eq!(c.press("s", &w2(), KEY, ms(2950)), Press::Fire);
        assert!(SETTLE >= Duration::from_millis(250) && SETTLE < CONFIRM);
    }

    #[test]
    fn a_cancel_sent_is_cancelling_until_the_rows_or_an_error_say_otherwise() {
        let t0 = Instant::now();
        let mut c = Cancels::default();
        c.sent("s", "sa_2", 7, t0);
        c.sent("s", "sa_3", 7, t0);
        assert!(c.pending("s", "sa_2") && !c.pending("t", "sa_2"));
        // The daemon took it: still cancelling until the rows say so.
        assert_eq!(c.answered(7, "sa_2", &Ok("Cancelled sa_2".into())), None);
        assert!(c.pending("s", "sa_2"));
        // An answer of another attachment is not this one's.
        assert_eq!(c.answered(8, "sa_3", &Err("x".into())), None);
        assert!(c.pending("s", "sa_3"));
        let note = c.answered(7, "sa_3", &Err("sa_3 is already completed — nothing to cancel".into()));
        assert_eq!(note.as_deref(), Some("sa_3 not cancelled: sa_3 is already completed — nothing to cancel"));
        assert!(!c.pending("s", "sa_3"));
        // The rows say it is cancelling: done here.
        assert!(!c.settle("s", |_| false, t0 + Duration::from_secs(1)));
        assert!(c.settle("s", |h| h == "sa_2", t0 + Duration::from_secs(1)));
        assert!(!c.pending("s", "sa_2"));
        // Never forever.
        c.sent("s", "sa_4", 7, t0);
        assert!(c.settle("x", |_| false, t0 + CANCEL_WAIT));
        assert!(!c.pending("s", "sa_4"));
    }

    fn acts(f: &Facts) -> Vec<(Act, bool)> {
        items(f).iter().map(|i| (i.act, i.enabled())).collect()
    }

    #[test]
    fn the_menu_says_what_can_be_done() {
        use Act::*;
        let f = facts();
        assert_eq!(
            acts(&f),
            vec![(Open, true), (OpenTab, true), (Steer, true), (Cancel, true), (CopyHandle, true), (CopyResult, false), (CopyTask, true)]
        );
        assert_eq!(items(&f)[4].label.as_ref(), "Copy handle (sa_2)");
        // Not this window's input: no steer, no cancel (as a steer), and why.
        let why = "read-only here: another client owns this session's input";
        let ro = Facts { read_only: Some(why), ..facts() };
        let a = acts(&ro);
        assert_eq!((a[2], a[3]), ((Steer, false), (Cancel, false)));
        assert_eq!(items(&ro)[3].why_not.as_deref(), Some(why));
        // Armed: the second press is asked for.
        let armed = items(&Facts { armed: true, ..facts() });
        assert_eq!((armed[3].label.as_ref(), armed[3].confirm.as_deref()), ("Cancel sleeper?", Some(crate::fonts::label::CONFIRM)));
        // Sent: cancelling, nothing to press.
        let sent = items(&Facts { pending: true, ..facts() });
        assert_eq!((sent[3].label.as_ref(), sent[3].enabled()), ("cancelling", false));
        // Finished with its whole result: copy it; no steer, no cancel.
        let done = Facts { state: Liveness::Done, result: Some((true, 900)), ..facts() };
        assert_eq!(acts(&done)[2..6], [(Steer, false), (Cancel, false), (CopyHandle, true), (CopyResult, true)]);
        assert_eq!(items(&done)[5].label.as_ref(), "Copy result");
        // M1: only a preview heard: it says so.
        let preview = items(&Facts { state: Liveness::Done, result: Some((false, 120)), ..facts() });
        assert_eq!((preview[5].label.as_ref(), preview[5].enabled()), ("Copy result preview (first 120 chars)", true));
        // A worker tab's menu: Show, no "in new tab".
        let tab = items(&Facts { tab: true, ..facts() });
        assert_eq!((tab[0].label.as_ref(), tab[1].act), ("Show", Steer));
    }

    /// H2: the menu of a worker whose handle names another worker now
    /// (a cold-started daemon) does nothing of that one's: no open, no
    /// steer, no cancel, no copy of its result or task.
    #[test]
    fn a_reused_handle_offers_nothing_of_the_other_worker() {
        use Act::*;
        let f = Facts { other: true, state: Liveness::Gone, result: Some((true, 40)), ..facts() };
        assert_eq!(
            acts(&f),
            vec![
                (Open, false),
                (OpenTab, false),
                (Steer, false),
                (Cancel, false),
                (CopyHandle, true),
                (CopyResult, false),
                (CopyTask, false)
            ]
        );
        let why = items(&f)[3].why_not.clone().unwrap();
        assert_eq!(why.as_ref(), "sa_2 is another worker now (the daemon restarted): sleeper is gone");
    }

    /// H2 with a cold-restart history: an old `scout` (sa_2, completed)
    /// and, after the restart, `builder` under sa_2 again (running). The
    /// old block is not the handle's worker; scout as its menu or a tab
    /// names it is another worker; builder is the handle's.
    #[test]
    fn a_cold_restart_history_names_each_worker_once() {
        let call = |id: &str, agent: &str, task: &str| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "subagent_start", "input": {"agent": agent, "task": task}}]});
        let answer = |id: &str, name: &str| {
            let text = json!({"handle_id": "sa_2", "agent_name": name, "status": "running"}).to_string();
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]})
        };
        let history = vec![
            call("t_old", "scout", "map it"),
            answer("t_old", "scout"),
            call("t_new", "builder", "build it"),
            answer("t_new", "builder"),
        ];
        let mut s = Subagents::default();
        s.reset(&history, false);
        assert!(!s.current("t_old") && s.current("t_new"), "sa_2 is builder's now");
        let now = s.ident(2);
        assert_eq!(now, Some((Some("t_new"), "builder")));
        // Scout as its block, a menu or a tab names it: another worker now.
        let scout = Who::new(2, "scout", Some("t_old".into()));
        let scout_by_name = Who::new(2, "scout", None);
        assert!(scout.other(now) && scout_by_name.other(now));
        // Builder: the handle's worker, by its call or by its name.
        assert!(!Who::new(2, "builder", Some("t_new".into())).other(now) && !Who::new(2, "builder", None).other(now));
        // A same-named worker after the restart: told apart by its call.
        assert!(!same_worker((Some("t_old"), "scout"), (Some("t_new"), "scout")));
        assert!(same_worker((None, "scout"), (Some("t_new"), "scout")), "no call to tell: the name decides");
        // Nothing known of the id: nobody else either.
        assert!(!scout.other(None));
    }
}
