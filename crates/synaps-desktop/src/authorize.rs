//! The authorize card in the log (polish.md §2.6, F3, F6, F7): a prompt's
//! own list row, directly after the call it gates (`waiting_on`: the last
//! unfinished call of the turn), or at the turn's end for a secret or a
//! confirm with no unfinished call. Flat on `card`, its 1 px `warn_mark`
//! state frame (traced on arrival, motion.md §6.3) and 2 px dashes along its
//! top; a header with what asked and when; the well; the folder; Allow all
//! (input owner only); DENY / ALLOW.
//!
//! Its state is one long-lived [`PromptCard`] per pending prompt, owned by
//! the [`AppView`] and only drawn by its row: the list builds rows while
//! they are visible, and the card's arm, read state, well scroll and the
//! secret field's text outlive that.
//!
//! Answered, the row collapses on `sheet` (P13, through
//! [`crate::motion::grow_block`]), its content at full strength until
//! clipped away, and the call's row says what happened in its meta:
//! `allowed 14:21 · 7s`, `denied 14:22`, `answered elsewhere 14:21`. A
//! prompt with no call row (a secret, a confirm with nothing unfinished)
//! keeps a one-line row: `secret · provided 14:21`, never the value.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use synaps_client::protocol::PromptRequest;
use synaps_client::transcript::{GateNote, Item, ToolCall, ToolStatus, gate_note};

use crate::app::{ARM, AppView, Arm, authorize_ready, hidden, lines_below, mono};
use crate::fonts;
use crate::fonts::TextPx as _;
use crate::live::Live;
use crate::model::CONFIRM_NO;
use crate::theme::{Metrics, Palette};
use crate::turns::{Disp, Kind};

/// A prompt row's id: its own count with this bit (item ids count up from 1;
/// `DAY_ID` is `1 << 63`).
pub(crate) const PROMPT_ID: u64 = 1 << 61;

/// The dashes along the card's top edge (F7): 8 on, 4 off.
// Q2's, kept while its tests stand: Cards D retires them (cards4/proposal.md §5, C13).
#[cfg_attr(not(test), allow(dead_code))]
const DASH: f32 = 8.;
#[cfg_attr(not(test), allow(dead_code))]
const DASH_GAP: f32 = 4.;
/// The allow-all box (drawn: 1 px frame, a 6×6 square when checked).
const CHECK: f32 = 12.;
const CHECK_IN: f32 = 6.;

/// Receipts kept per session (the oldest go first).
pub(crate) const RECEIPTS: usize = 64;

/// Now, local `HH:MM` (a receipt's time).
pub(crate) fn hhmm_now() -> SharedString {
    chrono::Local::now().format("%H:%M").to_string().into()
}

/// What the card's last live frame showed of its state: the collapse
/// draws the same, so nothing but its clip changes while it goes.
#[derive(Clone, Default)]
pub(crate) struct CardLook {
    pub(crate) can: bool,
    pub(crate) ready: bool,
    pub(crate) keys: bool,
    pub(crate) in_field: bool,
    pub(crate) typed: bool,
    pub(crate) footer: Option<SharedString>,
    pub(crate) footer_warn: bool,
    /// Allow all was offered: its line (a just-checked box too) stays
    /// through the collapse.
    pub(crate) offered: bool,
}

/// The prompt the card shows, kept while it is pending.
pub(crate) struct PromptCard {
    /// [`Model::card_key`]: `(epoch, prompt id)`.
    pub(crate) key: (u64, u64),
    /// Its list row's id ([`PROMPT_ID`] and a count: never reused).
    pub(crate) row: u64,
    /// A secret prompt's.
    pub(crate) secret: bool,
    pub(crate) arm: Arm,
    /// When this window first showed it: the receipt's "how long you
    /// took" counts from here (the arm restarts on a toggle or a return).
    pub(crate) shown: Instant,
    /// The prompt as it came (its title, its kind, its scope): what the
    /// card says while it collapses after the model has dropped it.
    pub(crate) pr: PromptRequest,
    /// The body, verbatim (blank lines kept).
    pub(crate) body: SharedString,
    /// What the top bar and the stub say about it ([`crate::chrome::prompt_gist`]).
    pub(crate) gist: SharedString,
    /// A guard's shape: the command alone in the well, the folder under it.
    pub(crate) guard: Option<Guard>,
    /// `bash · guard · asked 14:21` ([`asked`]).
    pub(crate) asked: Option<SharedString>,
    pub(crate) scroll: ScrollHandle,
    /// The well's bar: each card its own, so a drag on one never carries
    /// into the next.
    pub(crate) bar: crate::scrollbar::BarState,
    /// The buttons' line: where it was laid out ([`PromptCard::buttons_shown`]).
    pub(crate) actions: ScrollHandle,
    /// The well was scrolled to its end (or never needed to be), on screen.
    pub(crate) read: bool,
    /// Allow all is checked: ALLOW sends `"always"`.
    pub(crate) allow_all: bool,
    /// The Allow all box has the focus (Tab from the card's keys, or a
    /// click): Space toggles it then only (Shady M2). As this frame began.
    pub(crate) box_focus: bool,
    /// Its row's height as last drawn (what a collapse starts from).
    pub(crate) height: Rc<Cell<f32>>,
    /// The item its row follows (an id), and whether that is its own
    /// line once answered (no call row to carry the receipt).
    pub(crate) after: Cell<Option<(u64, bool)>>,
    /// Repaints once the arm delay is over.
    pub(crate) _arm: Option<Task<()>>,
}

impl PromptCard {
    /// Keep `card` with the prompt on top of `m` ([`Live::card_key`]): the
    /// same key keeps its arm and read state; a new prompt, the same id in
    /// another attachment or after a reconnect (a restarted daemon numbers
    /// its prompts from 1 again) starts over, unarmed and unread, at `now`.
    /// True when the card is new. `view`: what is on screen of the list,
    /// top and bottom (the window before it is laid out).
    pub(crate) fn sync(card: &mut Option<PromptCard>, m: &Live, now: Instant, view: (f32, f32)) -> bool {
        match (m.card_key(), card.as_mut()) {
            (Some(k), Some(c)) if c.key == k => {
                if c.below(view).is_some_and(|b| b <= 1.) {
                    c.read = true;
                }
                false
            }
            (Some(k), _) => {
                let pr = &m.transcript.pending_prompts()[0];
                *card = Some(PromptCard::new(k, pr, now));
                true
            }
            (None, _) => {
                *card = None;
                false
            }
        }
    }

    fn new(key: (u64, u64), pr: &PromptRequest, now: Instant) -> Self {
        static ROWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let secret = pr.is_secret();
        PromptCard {
            key,
            row: PROMPT_ID | ROWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            secret,
            arm: Arm::new(now),
            shown: now,
            body: crate::app::well_body(&pr.prompt),
            gist: crate::chrome::prompt_gist(&pr.title, &pr.prompt).into(),
            guard: guard_shape(pr),
            asked: asked(pr).map(Into::into),
            pr: pr.clone(),
            scroll: ScrollHandle::new(),
            bar: Default::default(),
            actions: ScrollHandle::new(),
            read: false,
            allow_all: false,
            box_focus: false,
            height: Rc::new(Cell::new(f32::NAN)),
            after: Cell::new(None),
            _arm: None,
        }
    }

    pub(crate) fn armed(&self) -> bool {
        self.arm.armed(Instant::now())
    }

    pub(crate) fn keys_armed(&self) -> bool {
        self.arm.keys_armed(Instant::now())
    }

    /// What of the well is out of sight with the list's view `top..view`,
    /// once it has been laid out: below its own view, under the list's
    /// bottom (the composer is not a place to read), or above the list's
    /// top (Shady M3: in the list the card's top can scroll away).
    pub(crate) fn below(&self, (top, view): (f32, f32)) -> Option<f32> {
        let b = self.scroll.bounds();
        if b.size.height <= px(0.) {
            return None;
        }
        let rest = f32::from(self.scroll.max_offset().y + self.scroll.offset().y);
        Some(unseen((f32::from(b.top()), f32::from(b.bottom())), rest, (top, view)))
    }

    /// Its buttons were laid out whole inside the list's view `top..view`
    /// (or not laid out yet: the frame that draws a new card).
    pub(crate) fn buttons_shown(&self, top: f32, view: f32) -> bool {
        let b = self.actions.bounds();
        b.size.height <= px(0.) || (f32::from(b.bottom()) <= view + 0.5 && f32::from(b.top()) >= top - 0.5)
    }
}

/// The answered card collapsing in its row (P13): the card as its last
/// frame drew it, with no listener (a collapse takes no click), clipped
/// from its height to `to` on `sheet`.
pub(crate) struct CardExit {
    pub(crate) card: PromptCard,
    pub(crate) look: CardLook,
    pub(crate) at: Instant,
    /// The session it was in (its row is that list's).
    pub(crate) session: String,
    /// The item its row follows.
    pub(crate) after: u64,
    /// Where its height lands: 0, or a receipt line's.
    pub(crate) to: f32,
    pub(crate) sink: Rc<Cell<f32>>,
    /// How it was answered (its button shows pressed as it folds).
    pub(crate) answer: Answer,
}

impl CardExit {
    /// Its shown height at `now` (`None`: not measured), and whether the
    /// collapse has landed.
    pub(crate) fn shown(&self, now: Instant) -> (Option<f32>, bool) {
        let from = self.card.height.get();
        if from.is_nan() {
            return (None, true);
        }
        // §8 #9: the height on `glide` from +40.
        let t = now.saturating_duration_since(self.at + FOLD_DELAY).as_secs_f32();
        let x = crate::motion::Spring::from_rest(1., crate::motion::GLIDE_RESPONSE, t);
        let h = self.to + (from - self.to).max(0.) * x;
        (Some(h), (h - self.to).abs() < 0.5)
    }

    /// Its strength as it folds (§8 #9): whole until +170, then its last
    /// sliver goes out over `quick` × 0.75.
    pub(crate) fn strength(&self, now: Instant) -> f32 {
        // The fold asks for every frame already: no timer of its own.
        let Some(t) = now.checked_duration_since(self.at + FOLD_FADE) else { return 1. };
        let k = (t.as_secs_f32() / crate::motion::exit(crate::motion::QUICK).as_secs_f32()).min(1.);
        1. - crate::motion::ease::IN.sample(k)
    }
}

/// When the answered card starts to fold, and when its last sliver fades
/// (cards4/proposal.md §8 #9: height `glide` @ +40, alpha `quick`×0.75 @ +170).
pub(crate) const FOLD_DELAY: Duration = Duration::from_millis(40);
pub(crate) const FOLD_FADE: Duration = Duration::from_millis(170);

impl CardExit {
    /// How far it has folded (0..), the room it gave up: the list keeps it
    /// at its end while the card folds, so the answered row holds still
    /// (the finish gate of #23, item 7).
    pub(crate) fn given(&self, now: Instant) -> f32 {
        let from = self.card.height.get();
        match self.shown(now).0 {
            Some(h) if !from.is_nan() => (from - h).max(0.),
            _ => 0.,
        }
    }
}

/// What became of a prompt (polish.md §2.6, the receipt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Allowed,
    AllowedAll,
    Denied,
    Provided,
    Cancelled,
    Elsewhere,
    /// Resolved with no answer from here, and nothing says who: neutral.
    Resolved,
}

/// An answered prompt's receipt, kept for its session while the window
/// lives (the daemon keeps no record of who answered when).
#[derive(Clone, Debug)]
pub(crate) struct Receipt {
    /// The call it belongs to: its daemon tool-use id (Shady H2: item ids
    /// are one transcript's, a reopened tab or a compaction renumbers
    /// them). In that call's row (its meta), or (`own`) a line after it.
    pub(crate) call: SharedString,
    pub(crate) own: bool,
    pub(crate) row: u64,
    pub(crate) secret: bool,
    pub(crate) answer: Answer,
    /// Local `HH:MM` of the answer.
    pub(crate) at: SharedString,
    /// How long the card waited for it here.
    pub(crate) took: Option<Duration>,
    /// When it landed: the receipt lifts in over `QUICK`.
    pub(crate) landed: Instant,
}

impl Receipt {
    /// `allowed 14:21 · 7s`, `denied 14:22`, `answered elsewhere 14:21`;
    /// a secret's `provided 14:21`. The first word and the rest.
    #[cfg(test)]
    pub(crate) fn words(&self) -> (&'static str, String) {
        self.words_while(false)
    }

    /// [`Receipt::words`]; while its call runs (`running`), without the
    /// wait: the row's clock beside it counts the run.
    pub(crate) fn words_while(&self, running: bool) -> (&'static str, String) {
        let took = self
            .took
            .filter(|_| !running && !matches!(self.answer, Answer::Elsewhere | Answer::Resolved))
            .map(|d| format!(" · {}", took(d)))
            .unwrap_or_default();
        (Self::word(self.answer), format!(" {}{took}", self.at))
    }

    /// What it says once its call's end is known (Shady L4): resolved with
    /// no answer from here, and the call cancelled, it was a cancel (a HALT
    /// from another client resolves the prompt while the turn still runs).
    /// A yes from here whose call shows it was held at its gate (#25:
    /// `applied`, [`applied`]) is `denied` (another client's Deny came
    /// first, the prompt timed out) or `cancelled` (a HALT crossed it, or
    /// one ran silently: the call can't say which), not `allowed`. That is
    /// an inference from the call, not the daemon's word: the answer sent
    /// stays in [`Receipt::answer`]. Read when drawn: no race.
    pub(crate) fn answer_for(&self, call: Option<ToolStatus>, applied: Option<bool>) -> Answer {
        match self.answer {
            Answer::Elsewhere | Answer::Resolved if call == Some(ToolStatus::Cancelled) => Answer::Cancelled,
            Answer::Allowed | Answer::AllowedAll | Answer::Provided if applied == Some(false) => {
                if call == Some(ToolStatus::Denied) { Answer::Denied } else { Answer::Cancelled }
            }
            a => a,
        }
    }

    /// The receipt's first word for `a`.
    pub(crate) const fn word(a: Answer) -> &'static str {
        match a {
            Answer::Allowed => "allowed",
            Answer::AllowedAll => "allowed all",
            Answer::Denied => "denied",
            Answer::Provided => "provided",
            Answer::Cancelled => "cancelled",
            Answer::Elsewhere => "answered elsewhere",
            Answer::Resolved => "resolved",
        }
    }
}

/// Whether a yes to the prompt that gated `call` got it past its gate, as
/// far as the call shows (#25). An inference, not an acknowledgement: a
/// daemon that doesn't say what it applied resolves the prompt with no
/// answer in it, and this window's answer is sent, not acknowledged
/// (another client's Deny, a HALT or the prompt's timeout can cross it).
///
/// `Some(true)`: it got past (`past`: [`Transcript::past_gate`], output
/// streamed or a result said it ran, kept once seen so a later bare cancel
/// or a settle doesn't take it back, Shady C64-1), or it ended on its own
/// (done, failed, stopped at its limit). `Some(false)`: its whole result is
/// one of the engine's own forms for a call held at its gate
/// ([`gate_note`]: the confirm's deny, a bare cancel), or the turn ended
/// before it reported and nothing streamed. A bare cancel is also what a
/// call that ran silently gets: it reads `cancelled`, unlit, not proven
/// either way. `None`: not known (queued, running, silent; any other
/// result). Tool output is never searched for a phrase (Shady C64-2); a
/// tool whose whole output is byte for byte a gate form reads as one.
///
/// [`Transcript::past_gate`]: synaps_client::Transcript::past_gate
pub(crate) fn applied(call: Option<&ToolCall>, past: bool) -> Option<bool> {
    if past {
        return Some(true);
    }
    let t = call?;
    match t.status? {
        ToolStatus::Done | ToolStatus::Failed | ToolStatus::Unknown => Some(true),
        s @ (ToolStatus::Denied | ToolStatus::Cancelled) => match gate_note(&t.result, &t.tail) {
            Some(GateNote::Denied | GateNote::BareCancel) => Some(false),
            Some(GateNote::StartedCancel) => Some(true),
            None => (s == ToolStatus::Cancelled && t.result.is_empty()).then_some(false),
        },
        // Queued, running, streaming (the status is `non_exhaustive`).
        _ => None,
    }
}

/// When a call's run started, for its clock: a gated call's at its answer
/// (the wait before it is the receipt's, not the run's).
pub(crate) fn run_start(started: Option<Instant>, receipt: Option<&Receipt>) -> Option<Instant> {
    started.max(receipt.map(|r| r.landed))
}

/// `7s`, `2m 05s`, `1h 03m`.
pub(crate) fn took(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// An answer this window sent, until its prompt resolves (or the daemon
/// refuses it: then it is forgotten and the card asks again).
#[derive(Clone, Debug)]
pub(crate) struct Sent {
    pub(crate) key: (u64, u64),
    pub(crate) answer: Answer,
    pub(crate) at: SharedString,
    pub(crate) took: Duration,
}

/// A guard's confirm (polish.md §2.6, the well): a `$ ` command line after
/// a line that introduces it with `… in <folder>:`, read as
/// [`crate::chrome::prompt_gist`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Guard {
    pub(crate) command: SharedString,
    pub(crate) dir: SharedString,
}

///
/// Only for a confirm the daemon framed cleanly (Shady H1): it has a scope,
/// no extension rewrote the input (no `Input rewritten by` / `Input that
/// will run`: the well must show what runs, so such a prompt is shown
/// whole), and the daemon's header before `Message from …:` is exactly its
/// `Extension:` and `Tool:` lines. The well then shows the message whole
/// after its intro, every line up to `Allow this tool call?`.
pub(crate) fn guard_shape(pr: &PromptRequest) -> Option<Guard> {
    let scope = pr.confirm_scope.as_ref()?;
    if !scope.rewritten_by.is_empty() || pr.is_secret() {
        return None;
    }
    let lines: Vec<&str> = pr.prompt.lines().map(str::trim_end).collect();
    let from = lines.iter().position(|l| l.starts_with("Message from ") && l.ends_with(':'))?;
    let head: Vec<&str> = lines[..from].iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if head.len() != 2 || !head[0].starts_with("Extension: ") || !head[1].starts_with("Tool: ") {
        return None;
    }
    let first_after = |at: usize| (at + 1..lines.len()).find(|&i| !lines[i].trim().is_empty());
    let intro = first_after(from)?;
    let start = first_after(intro)?;
    let command = lines[start].trim_start().strip_prefix("$ ")?;
    let end = lines.iter().rposition(|l| l.trim() == "Allow this tool call?").filter(|&e| e > start).unwrap_or(lines.len());
    let mut rest: Vec<&str> = lines[start + 1..end].to_vec();
    while rest.last().is_some_and(|l| l.trim().is_empty()) {
        rest.pop();
    }
    let command = std::iter::once(command).chain(rest).collect::<Vec<_>>().join("\n");
    let dir = lines[intro].trim().strip_suffix(':')?.rsplit_once(" in ")?.1.trim();
    (!dir.is_empty() && !command.trim().is_empty()).then(|| Guard { command: command.into(), dir: dir.to_string().into() })
}

/// A call row's status word beside its receipt: none when the receipt
/// already says it, so the meta never reads `denied denied 14:22` or
/// `cancelled cancelled 19:20`.
pub(crate) fn status_beside(word: Option<&'static str>, receipt: Option<Answer>) -> Option<&'static str> {
    word.filter(|w| receipt.is_none_or(|a| !Receipt::word(a).eq_ignore_ascii_case(w)))
}

/// The header's right: `bash · guard · asked 14:21`, from the scope's tool
/// and extensions and `raised_at` (local, 24 h); a missing part goes with
/// its separator.
pub(crate) fn asked(pr: &PromptRequest) -> Option<String> {
    let scope = pr.confirm_scope.as_ref();
    let parts: Vec<String> = [
        scope.and_then(|s| s.tool.clone()).filter(|t| !t.is_empty()),
        scope.map(|s| s.extensions_label()).filter(|e| !e.is_empty()),
        pr.raised_at.as_deref().and_then(crate::times::Stamp::local).map(|t| format!("asked {}", t.hhmm)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A prompt's row in the list: after item `after` (an index), its id and
/// rev (a receipt line's differs from the card's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PromptRow {
    pub(crate) after: usize,
    pub(crate) id: u64,
    pub(crate) rev: u64,
}

/// `out` (the rows of `items[from..]`) with the prompt rows that follow
/// those items, each after the last row of its item (after a folded run's
/// summary when its item is in the run).
pub(crate) fn with_prompts(mut out: Vec<Disp>, prompts: &[PromptRow], from: usize) -> Vec<Disp> {
    for r in prompts.iter().rev().filter(|r| r.after >= from) {
        let at = out.partition_point(|d| d.item <= r.after);
        out.insert(
            at,
            Disp { item: r.after, id: r.id, rev: r.rev, kind: Kind::Prompt, gap: crate::turns::Gap::None, who: crate::turns::Who::None },
        );
    }
    out
}

/// The prompt rows in `disp`, as built.
pub(crate) fn built(disp: &[Disp]) -> Vec<PromptRow> {
    disp.iter().filter(|d| d.kind == Kind::Prompt).map(|d| PromptRow { after: d.item, id: d.id, rev: d.rev }).collect()
}

/// The index of item `id` in `items` (ids are not sorted: notes keep
/// theirs, a refold reuses some).
fn index_of(items: &[Item], id: u64) -> Option<usize> {
    items.iter().rposition(|x| x.id == id)
}

/// The index of the call with daemon tool-use id `call` in `items`.
pub(crate) fn call_ix(items: &[Item], call: &str) -> Option<usize> {
    items.iter().rposition(|x| x.tool.as_ref().is_some_and(|t| t.tool_id == call))
}

impl AppView {
    /// The prompt rows the list should hold now: the live card's (after the
    /// call it waits on, else at the end), the collapsing card's, and the
    /// receipt lines of this session's own-row prompts. None over a
    /// worker's view.
    pub(crate) fn prompt_rows(&self) -> Vec<PromptRow> {
        let mut out = Vec::new();
        if self.model.lives[self.panes.cur.live].watch.focus().is_some() {
            return out;
        }
        let Some(sid) = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str()) else { return out };
        let items = self.model.lives[self.panes.cur.live].shown().items();
        if items.is_empty() {
            return out;
        }
        // The collapsing card's row becomes its receipt line when it lands.
        let exit = self.panes.cur.card_exit.as_ref().filter(|e| e.session == sid);
        for r in self.receipts.get(sid).into_iter().flatten().filter(|r| r.own && exit.is_none_or(|e| e.card.row != r.row)) {
            // Its call gone (compacted away): no line, never a wrong one.
            if let Some(after) = call_ix(items, &r.call) {
                out.push(PromptRow { after, id: r.row, rev: 1 });
            }
        }
        if let Some(e) = exit
            && let Some(after) = index_of(items, e.after)
        {
            out.push(PromptRow { after, id: e.card.row, rev: 0 });
        }
        if let Some(c) = self.current_card() {
            // The call it gates, or the last item.
            let call = self.panes.cur.waiting_on.filter(|_| !c.secret).and_then(|id| index_of(items, id));
            let after = call.unwrap_or(items.len() - 1);
            c.after.set(Some((items[after].id, call.is_none())));
            out.push(PromptRow { after, id: c.row, rev: 0 });
        }
        out.sort();
        out
    }

    /// The card's row in the list, if built.
    pub(crate) fn card_ix(&self) -> Option<usize> {
        let row = self.current_card()?.row;
        self.panes.cur.disp.iter().rposition(|d| d.kind == Kind::Prompt && d.id == row)
    }

    /// The list's view, top and bottom in the window (the window when it
    /// has not been laid out).
    /// Read from the last layout as a frame begins ([`AppView::read_list_view`]):
    /// a row drawing inside the list's layout may not ask the list.
    pub(crate) fn list_view(&self) -> (f32, f32) {
        self.panes.cur.list_box.get()
    }

    /// The list's view as its last layout left it (as a frame begins).
    pub(crate) fn read_list_view(&self) {
        let b = self.panes.cur.list.viewport_bounds();
        let v = if b.size.height <= px(0.) { (0., self.height) } else { (f32::from(b.top()), f32::from(b.bottom())) };
        self.panes.cur.list_box.set(v);
    }

    /// The card can be answered from the screen: its row is in the list's
    /// view with its buttons whole (as the frame began: [`AppView::read_card_on_screen`]).
    pub(crate) fn card_on_screen(&self) -> bool {
        self.panes.cur.card_seen.get()
    }

    /// [`AppView::card_on_screen`] from the list's last layout, as a frame
    /// begins (a row drawing inside the list's layout may not ask the list).
    pub(crate) fn read_card_on_screen(&self) {
        self.panes.cur.card_seen.set(self.card_on_screen_now());
    }

    fn card_on_screen_now(&self) -> bool {
        let Some(c) = self.current_card() else { return false };
        let (top, bottom) = self.list_view();
        let Some(ix) = self.card_ix() else { return false };
        match self.panes.cur.list.bounds_for_item(ix) {
            Some(b) => f32::from(b.top()) < bottom && f32::from(b.bottom()) > top && c.buttons_shown(top, bottom),
            // Not drawn yet: the frame that draws a new card at the end,
            // where the list follows, counts it shown, as its buttons do.
            // Only as the last row (Shady M6): one placed above others is
            // virtualised away, unseen.
            None => c.height.get().is_nan() && self.panes.cur.list.is_following_tail() && ix + 1 == self.panes.cur.disp.len(),
        }
    }

    /// Ctrl+J: the card's row into view (on F1's glide when it was drawn,
    /// at Full; at once when not, and at Reduced and Off), from a worker's view back to the session first,
    /// and the keys to it (a secret's to its field).
    /// Split (split.md §7, A14): nothing waits in the focused pane, the
    /// first pane in reading order whose visible session waits is focused
    /// and its card shown.
    pub(crate) fn jump_to_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.lives[self.panes.cur.live].transcript.pending_prompts().is_empty() {
            if let Some(id) = self.waiting_pane() {
                self.focus_pane(id, crate::pane::KeysTo::Follow, cx);
                self.reveal_card(window, cx);
            }
            return;
        }
        if self.model.tabs.view().is_some() {
            return;
        }
        if self.model.lives[self.panes.cur.live].watch.focus().is_some() {
            self.open_parent_card(cx);
            self.panes.cur.jump_card = self.model.lives[self.panes.cur.live].card_key();
            cx.notify();
            return;
        }
        self.reveal_card(window, cx);
    }

    /// The card's row in view and the keys to it (after a frame has built
    /// it, [`Pane::jump_card`]).
    pub(crate) fn reveal_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.card_ix() else { return };
        let (top, bottom) = self.list_view();
        // Below the sticky band, when the labels are inline (polish.md F8);
        // above the dock as it will be once it lands (polish.md P13): the
        // stub leaves now (it pointed here), its room is the view's.
        let top = top + self.sticky_inset(cx);
        let leaves = if self.stub_shown() && self.model.lives[self.panes.cur.live].watch.focus().is_none() {
            self.panes.cur.stub_away = self.model.lives[self.panes.cur.live].card_key();
            crate::fit::STUB_GAP + crate::theme::metrics(cx).row
        } else {
            0.
        };
        let bottom = bottom - self.dock_growth() + leaves;
        match self.panes.cur.list.bounds_for_item(ix) {
            Some(b) => {
                let (t, bt) = (f32::from(b.top()), f32::from(b.bottom()));
                // Its bottom to the view's (its top if it is taller).
                let by = if t < top || bt - t > bottom - top {
                    t - top
                } else if bt > bottom {
                    bt - bottom
                } else {
                    0.
                };
                if by.abs() > 0.5 {
                    self.panes.cur.list.pause_following_tail();
                    // Reduced and Off: a cut, in this frame (the stub goes
                    // with it); there is no glide to play.
                    let cut = if self.fx.level() == crate::motion::Level::Full {
                        self.panes.cur.scroller.jump(by, false, bottom - top, self.fx.clock())
                    } else {
                        by
                    };
                    if cut != 0. {
                        self.panes.cur.scroller.touch();
                        self.panes.cur.list.scroll_by(px(cut));
                    }
                }
            }
            None => {
                // The user's jump (Shady #23 r4 H1): it leaves the end.
                self.panes.cur.scroller.touch();
                self.panes.cur.list.scroll_to_reveal_item(ix);
                // Drawn next frame: placed again then, the dock as it lands.
                if self.dock_growth() > 0.5 {
                    self.panes.cur.jump_card = self.model.lives[self.panes.cur.live].card_key();
                }
            }
        }
        let secret = self.current_card().is_some_and(|c| c.secret);
        if secret {
            self.panes.cur.secret_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Space on the focused box, or a click: Allow all on or off (when
    /// offered, and the card may be answered from here now). Its buttons
    /// and keys wait again (Shady M2: a toggle and an ALLOW are never one
    /// motion).
    pub(crate) fn toggle_allow_all(&mut self, cx: &mut Context<Self>) {
        let offered = self.allow_all_offered();
        let may = self.card_answerable().is_some()
            && self.panes.cur.card.as_ref().is_some_and(|c| !self.model.lives[self.panes.cur.live].prompt_answered(c.key.1));
        if let Some(c) = self.panes.cur.card.as_mut().filter(|_| offered && may) {
            c.allow_all = !c.allow_all;
            c.arm = Arm::new(Instant::now());
            c._arm = Some(Self::arm_repaint(cx));
            log::info!(target: "synaps_desktop::card", "card {:?}: allow all {}", c.key, if c.allow_all { "on" } else { "off" });
            cx.notify();
        }
    }

    /// Allow all is offered on the card: its prompt has a scope to latch,
    /// and this client owns the session's input (the daemon refuses
    /// `"always"` from anyone else: `actor.rs confirm_answer_refusal`).
    pub(crate) fn allow_all_offered(&self) -> bool {
        let owner = self.model.lives[self.panes.cur.live].active.as_ref().is_some_and(|a| a.owner == Some(a.client));
        owner && self.current_card().is_some_and(|c| c.pr.allow_all().is_some())
    }

    /// A call row's receipt (its meta's last part, after a ` · ` when
    /// something comes before it), lifting in over `QUICK`. While its call
    /// runs, without the wait (the clock beside it counts the run).
    pub(crate) fn receipt_el(
        &self,
        call: Option<&str>,
        status: Option<ToolStatus>,
        after: bool,
        running: bool,
        p: &Palette,
    ) -> Option<AnyElement> {
        if self.model.lives[self.panes.cur.live].watch.focus().is_some() {
            return None;
        }
        let r = self.row_receipt(call)?;
        let applied = self.call_applied(&r.call);
        let words = self.receipt_words(r, r.answer_for(status, applied), applied == Some(true), running, p);
        let row = r.row;
        // The meta's 8 px gap is the separator's first space.
        // Cards D: the facts' dot before it (cards4/proposal.md §3).
        Some(if after {
            h_flex()
                .gap(px(6.))
                .items_center()
                .child(crate::cards::dot(p).debug_selector(move || format!("receipt-sep {row}")))
                .child(words)
                .into_any_element()
        } else {
            words.into_any_element()
        })
    }

    /// The receipt the row of call `call` (its tool-use id) carries in its
    /// meta, if any.
    pub(crate) fn row_receipt(&self, call: Option<&str>) -> Option<&Receipt> {
        let call = call?;
        let sid = self.model.lives[self.panes.cur.live].active.as_ref()?.session_id.as_str();
        self.receipts.get(sid)?.iter().rev().find(|r| !r.own && r.call == call)
    }

    /// [`applied`] for call `call` (its tool-use id), from the transcript.
    pub(crate) fn call_applied(&self, call: &str) -> Option<bool> {
        let t = &self.model.lives[self.panes.cur.live].transcript;
        let items = t.items();
        applied(call_ix(items, call).and_then(|i| items[i].tool.as_ref()), t.past_gate(call))
    }

    /// The receipt's words; `applied`: its call shows the daemon applied
    /// the answer ([`applied`]).
    fn receipt_words(&self, r: &Receipt, answer: Answer, applied: bool, running: bool, p: &Palette) -> Div {
        let (_, rest) = r.words_while(running);
        let word = Receipt::word(answer);
        let k = self.fx.shot(Some(r.landed), Duration::ZERO, crate::motion::QUICK, &crate::motion::ease::OUT, crate::motion::Kind::Fade);
        let lift = crate::motion::LIFT + (1. - crate::motion::LIFT) * k;
        // Cards D (cards4/proposal.md §8 #9): an Allow's receipt is lit in
        // the accent as it lands and decays to `dim` (`glow` from +240); a
        // Deny's stays `err`. Nothing else lights (Shady #23 L9: attention
        // follows the accent; a cancel, another client's answer, `dim`).
        // Only once its call shows the daemon applied it (#25): until then
        // `dim`, and if it never does, it is not an Allow.
        let lights = applied && matches!(answer, Answer::Allowed | Answer::AllowedAll | Answer::Provided);
        let lit = if lights { self.fx.decay(Some(r.landed + Duration::from_millis(240)), Duration::ZERO, crate::motion::GLOW) } else { 0. };
        #[cfg(test)]
        RECEIPT_LIT.with(|l| l.borrow_mut().push((r.row, Receipt::word(answer), lit > 0.)));
        let rest_color = crate::motion::mix(p.dim_t, p.acc, lit);
        let word_color = if answer == Answer::Denied { p.err } else { rest_color };
        let row = r.row;
        // Sentence case: "Allowed 08:31 · 2s", "Denied 14:22".
        let shown: SharedString = {
            let mut c = word.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default().into()
        };
        crate::motion::lift(
            h_flex()
                .debug_selector(move || format!("receipt {row}"))
                .flex_none()
                .whitespace_nowrap()
                .font_family(fonts::SANS)
                .child(div().debug_selector(move || format!("receipt-word {row} {word}")).text_color(word_color).child(shown))
                .child(div().text_color(if answer == Answer::Denied { p.err } else { rest_color }).child(SharedString::from(rest))),
            lift,
        )
    }

    /// A prompt row ([`Kind::Prompt`]): the live card, the answered card
    /// collapsing, or an own-row receipt line.
    pub(crate) fn render_authorize(&self, d: Disp, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> AnyElement {
        if let Some(c) = self.current_card().filter(|c| c.row == d.id) {
            let height = c.height.clone();
            return div()
                .w_full()
                .child(card_slot(self.render_card(c, None, p, m, cx), m))
                .on_children_prepainted(move |b, _, _| {
                    if let Some(b) = b.first() {
                        height.set(f32::from(b.size.height));
                    }
                })
                .into_any_element();
        }
        if let Some(e) = self.panes.cur.card_exit.as_ref().filter(|e| e.card.row == d.id) {
            let body = card_slot(self.render_card(&e.card, Some(&e.look), p, m, cx), m).into_any_element();
            let (shown, _) = e.shown(self.fx.now());
            self.fx.want_frame();
            let g =
                crate::motion::Grow { key: d.id, shown, sink: e.sink.clone(), travel: 0., strength: e.strength(self.fx.now()), floor: 0. };
            return crate::motion::grow_block(body, None, None, g);
        }
        let sid = self.model.lives[self.panes.cur.live].active.as_ref().map(|a| a.session_id.as_str()).unwrap_or_default();
        let Some(rc) = self.receipts.get(sid).and_then(|v| v.iter().find(|x| x.row == d.id)) else { return div().into_any_element() };
        let kind = if rc.secret { "secret" } else { "confirm" };
        let items = self.model.lives[self.panes.cur.live].transcript.items();
        let status = call_ix(items, &rc.call).and_then(|i| items[i].tool.as_ref()).and_then(|t| t.status);
        let applied = self.call_applied(&rc.call);
        h_flex()
            .w_full()
            .h(px(receipt_line(m)))
            .items_center()
            .text_px(m.meta)
            .font_family(fonts::MONO)
            .whitespace_nowrap()
            .debug_selector(move || format!("receipt-row {}", d.id))
            // On the call rows' verb column (the finish gate of #18).
            .child(div().pl(px(crate::turns::Geo::of(self.panes.cur.layout, m).verb)).text_color(p.dim_t).child(format!("{kind} · ")))
            .child(self.receipt_words(rc, rc.answer_for(status, applied), applied == Some(true), false, p))
            .into_any_element()
    }

    /// The card. `look`: the collapsing card's last live frame (no
    /// listener, its state as it was); `None`: the live card.
    fn render_card(&self, c: &PromptCard, look: Option<&CardLook>, p: &Palette, m: &Metrics, cx: &mut Context<Self>) -> Stateful<Div> {
        let r = m.r;
        let live = look.is_none();
        let pr = &c.pr;
        let secret = c.secret;
        let sent = live && self.model.lives[self.panes.cur.live].prompt_answered(pr.id);
        let armed = live && c.armed();
        let keys_state = if live { self.panes.cur.card_keys.get() } else { None };
        let can = look.map_or_else(|| self.drives() && self.model.lives[self.panes.cur.live].may_answer(pr.id) && armed, |l| l.can);
        let below = c.below(self.list_view());
        let ready = look.map_or_else(|| authorize_ready(c.armed(), below, c.read), |l| l.ready);
        let detached = self.model.lives[self.panes.cur.live].active.is_none();
        let in_field = look.map_or(keys_state == Some(true), |l| l.in_field);
        let keys = look.map_or_else(|| keys_state.is_some() && can && c.keys_armed(), |l| l.keys);
        let typed = look.map_or_else(|| secret && !self.panes.cur.secret_input.read(cx).value().is_empty(), |l| l.typed);
        let key = live.then_some(c.key);
        let offered = look.map_or_else(|| self.allow_all_offered(), |l| l.offered);
        let allow_all = offered && c.allow_all;
        // Cards D's buttons (cards4/proposal.md §5): Deny a ghost (`card_d`
        // in a `line` edge), Allow the accent ([`ALLOW_STYLE`]), each with
        // its key at 72 %. Pressed (the collapse's first 90 ms, §8 #9): down
        // `PRESS`, Allow's fill 18 % toward the ink, Deny's `hover`.
        // The answer's button stays pressed from the click (or its key) to
        // the end of the fold (Shady #23 M1, the finish gate's item 9): down
        // while it waits for the daemon (the footer says so), and as the
        // card folds, never back to its rest or disabled look between.
        let pressed = match look {
            None => self.sent.as_ref().filter(|s| s.key == c.key).map(|s| s.answer),
            Some(_) => self.panes.cur.card_exit.as_ref().filter(|e| e.card.row == c.row).map(|e| e.answer),
        };
        let tight = self.panes.cur.layout.tight;
        let key_hint = |k: &'static str, color: Hsla, ground: Hsla| {
            div().flex_none().font_family(fonts::SANS).text_px(m.meta).text_color(crate::motion::mix(ground, color, 0.72)).child(k)
        };
        let ghost = |id: &'static str, text: &'static str, key: Option<&'static str>, enabled: bool, down: bool| {
            // Pressed, its label stays (Shady #23 r2 L2): as Allow's.
            let fg = if enabled || down { p.ink } else { p.dim_k };
            let bg = if down { p.hover } else { p.card_d };
            h_flex()
                .id(id)
                .relative()
                .when(down, |d| d.top(px(crate::motion::PRESS)))
                .flex_none()
                .h(px(crate::fit::button_h(m)))
                .px(px(12.))
                .gap(px(8.))
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(p.line)
                .bg(bg)
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(p.hover)))
                // Pressed under the pointer (fluid.md §3.9): state, in the
                // frame it goes down, at every level.
                .when(enabled, |d| d.active(|s| s.bg(p.hover).top(px(crate::motion::PRESS))))
                .child(div().font_family(fonts::SANS).font_weight(FontWeight::SEMIBOLD).text_px(m.label).text_color(fg).child(text))
                .children(key.map(|k| key_hint(k, fg, bg)))
        };
        type Key = Option<&'static str>;
        let allow_btn = |id: &'static str, text: &'static str, key: Key, enabled: bool, charge: Option<u32>, down: bool| {
            let style = allow_style();
            let (bg, edge, fg) = style.colors(p, false, down);
            let (hi, _, _) = style.colors(p, true, false);
            let (press, press_edge, _) = style.colors(p, false, true);
            let (bg, edge, fg) = if enabled || down { (bg, edge, fg) } else { (p.card_d, p.line, p.dim_k) };
            // The arm's charge in the accent, filled or outlined (Shady
            // #23 M5: the outline's rest fill is the card itself).
            let lit = match style {
                crate::cards::AllowStyle::Fill => style.colors(p, false, false).0,
                crate::cards::AllowStyle::Outline => p.acc_line,
            };
            h_flex()
                .id(id)
                .relative()
                .when(down, |d| d.top(px(crate::motion::PRESS)))
                .flex_none()
                .h(px(crate::fit::button_h(m)))
                .px(px(14.))
                .gap(px(8.))
                .items_center()
                .rounded(px(5.))
                .overflow_hidden()
                .border_1()
                .border_color(edge)
                .bg(bg)
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(move |d| d.bg(hi).border_color(if style == crate::cards::AllowStyle::Fill { hi } else { edge }))
                        .active(move |s| s.bg(press).border_color(press_edge).top(px(crate::motion::PRESS)))
                })
                // The arm's charge (motion.md §6.3): ten cells of the fill
                // lit left to right along the foot, as before.
                .children(charge.filter(|_| !enabled).map(|n| {
                    h_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(3.))
                        .gap(px(1.))
                        .children((0..10).map(move |i| div().flex_1().h_full().when(i < n, |d| d.bg(lit))))
                }))
                .child(div().font_family(fonts::SANS).font_weight(FontWeight::SEMIBOLD).text_px(m.label).text_color(fg).child(text))
                .children(key.map(|k| key_hint(k, fg, bg)))
                .debug_selector(move || if style == crate::cards::AllowStyle::Fill { "allow-fill".into() } else { "allow-outline".into() })
        };
        let actions = if detached {
            None
        } else if secret {
            Some(
                h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px((120. * r).round()))
                            .max_w(px((260. * r).round() + 64.))
                            .gap(px(8.))
                            .items_center()
                            .child(
                                div().flex_1().min_w_0().max_w(px((260. * r).round())).debug_selector(|| "card-secret".into()).child(
                                    gpui_kit::component::input::Input::new(&self.panes.cur.secret_input)
                                        .disabled(!can)
                                        .text_px(m.mono)
                                        .border_color(p.line),
                                ),
                            ),
                    )
                    .child(
                        ghost("p-cancel", "Cancel", keys.then_some("Esc"), can, pressed == Some(Answer::Cancelled))
                            .when(live && can, |d| d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(false, window, cx)))),
                    )
                    .child(
                        allow_btn(
                            "p-submit",
                            "Submit",
                            (in_field && can && typed).then_some(if cfg!(target_os = "macos") { "↵" } else { "Enter" }),
                            can && typed,
                            None,
                            pressed == Some(Answer::Provided),
                        )
                        .when(live && can && typed, |d| {
                            d.on_click(cx.listener(|this, _, window, cx| this.answer_secret(true, window, cx)))
                        }),
                    ),
            )
        } else {
            let yes = can && ready;
            // ALLOW charges (motion.md §6.3): the arm's wait drawn as 10
            // cells lit left to right, 50 ms each; the last lit is the arm.
            // A held read gate keeps them dark.
            let charge = Some(c)
                .filter(|c| {
                    let may = self.drives() && self.model.lives[self.panes.cur.live].may_answer(pr.id);
                    live && may && !yes && self.fx.level() == crate::motion::Level::Full && !(below.is_some_and(|b| b > 1.) && !c.read)
                })
                .map(|c| {
                    let cell = ARM / 10;
                    let n = (self.fx.now().saturating_duration_since(c.arm.shown_at()).as_nanos() / cell.as_nanos()).min(10) as u32;
                    if n < 10 {
                        self.fx.want_at(c.arm.shown_at() + cell * (n + 1));
                    }
                    n
                });
            let allow_key = if cfg!(target_os = "macos") { "⌘ ↵" } else { "Ctrl ↵" };
            Some(
                h_flex()
                    .gap(px(8.))
                    .items_center()
                    .child(ghost("p-no", "Deny", keys.then_some("Esc"), can, pressed == Some(Answer::Denied)).when_some(
                        key.filter(|_| can),
                        |d, key| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.answer(key, Some(CONFIRM_NO.into()));
                                cx.notify();
                            }))
                        },
                    ))
                    // Clickable whenever the card may be answered: the read
                    // gate is checked at the click (as the keys do), not by
                    // this frame, laid out with the last one's bounds.
                    .child(
                        allow_btn(
                            "p-yes",
                            "Allow",
                            (keys && yes).then_some(allow_key),
                            yes,
                            charge,
                            matches!(pressed, Some(Answer::Allowed | Answer::AllowedAll)),
                        )
                        .when_some(key.filter(|_| can), |d, key| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.authorize(key);
                                cx.notify();
                            }))
                        }),
                    ),
            )
        };
        let more = below.filter(|&b| b > 1.).map(|b| lines_below(b, m.code_line)).filter(|_| !c.read);
        let prompts = self.model.lives[self.panes.cur.live].transcript.pending_prompts();
        let footer: Option<SharedString> = if let Some(l) = look {
            l.footer.clone()
        } else if detached && self.opening() {
            Some("opening…".into())
        } else if detached {
            Some("detached: cannot answer".into())
        } else if sent {
            Some("answer sent, waiting for the daemon".into())
        } else if self.model.lives[self.panes.cur.live].switching() {
            Some("switching sessions…".into())
        } else if !self.drives() {
            Some(if self.client.is_none() { "offline: nothing to answer" } else { "read-only here" }.into())
        } else if !secret && let Some(n) = more {
            Some(
                if keys {
                    format!("↓ {n} more lines: PgDn or scroll to the end")
                } else {
                    format!("↓ {n} more lines: scroll to the end to allow")
                }
                .into(),
            )
        } else if prompts.len() > 1 {
            Some(format!("+{} more waiting", prompts.len() - 1).into())
        } else {
            None
        };
        let footer_warn = look.map_or(more.is_some() && !sent, |l| l.footer_warn);
        if live {
            *self.panes.cur.card_look.borrow_mut() =
                CardLook { can, ready, keys, in_field, typed, footer: footer.clone(), footer_warn, offered };
        }
        // The command strip (§5): a guard's command alone, `$` in `dim`;
        // else the whole body, verbatim. Up to the form's most lines, the
        // rest scrolls (and Allow waits for its end).
        // As many whole lines as the window has room for beside the
        // composer (fit.rs: from 3, fewer if the text has fewer, to the
        // form's most), so the whole card fits the list's view.
        let form = self.panes.cur.layout.form();
        let shown_lines = match &c.guard {
            Some(g) => g.command.lines().count().max(1) as f32,
            None => c.body.lines().count().max(1) as f32,
        };
        let ex = self.extras();
        let floor = crate::fit::well_floor(m, form, shown_lines.clamp(1., crate::fit::WELL_MIN), ex);
        let rows = self.panes.cur.composer.read(cx).text().lines_len().clamp(1, self.panes.cur.composer_rows.max(1));
        // A soft-wrapped line takes more than its one: the most is the
        // window's room, the least the text's own lines (they scroll).
        let lines = crate::fit::well_lines(m, form, self.height, rows, floor, ex);
        let well_pad = (8. * r).round();
        let well_h = |lines: f32| lines * m.code_line + 2. * well_pad;
        let text: AnyElement = match &c.guard {
            Some(g) => h_flex()
                .items_start()
                .child(div().flex_none().text_color(p.dim_t).child("$ "))
                .child(div().min_w_0().child(g.command.clone()))
                .into_any_element(),
            None => div().child(c.body.clone()).into_any_element(),
        };
        let has_well = !pr.prompt.is_empty();
        // Above and below the text, a border in the well's own colour, not
        // padding: GPUI clips a scroll to inside its border, so no sliver of
        // the next line shows under the last whole one.
        let well = div()
            .id("prompt-well")
            .debug_selector(|| "prompt-well".into())
            .flex_none()
            .min_h(px(well_h(floor.min(shown_lines))))
            .max_h(px(well_h(lines)))
            .overflow_y_scroll()
            .track_scroll(&c.scroll)
            .px(px((12. * r).round()))
            .map(|mut d| {
                let b = &mut d.style().border_widths;
                b.top = Some(px(well_pad).into());
                b.bottom = Some(px(well_pad).into());
                d
            })
            .border_color(p.well_d)
            .bg(p.well_d)
            .font_family(fonts::MONO)
            .text_px(m.mono)
            .line_height(px(m.code_line))
            .text_color(p.ink)
            .child(text);
        // The card asks once (§8 #8): its flush, 12 % of the accent off the
        // card, and its header dot's one ping, from +60 over `sig`; then
        // still until answered. Collapsing, as its last frame.
        let arrived = self.panes.cur.card_at.filter(|_| live);
        let flush = self.fx.decay(arrived.map(|a| a + 2 * crate::motion::STAGGER), Duration::ZERO, crate::motion::SIG);
        let ping = self.fx.ping(arrived, 2 * crate::motion::STAGGER);
        let card_bg = crate::motion::mix(p.card_d, p.ask_flush, flush);
        let dot = div()
            .relative()
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(6.)).rounded(px(3.)).bg(p.acc))
            .children(ping.map(|k| crate::glyph::ping(k, p.acc, card_bg)))
            .debug_selector(|| "card-dot".into());
        let title = match (secret, &c.guard) {
            (true, _) => "Provide a secret?",
            (false, Some(_)) => "Run this command?",
            (false, None) => "Allow this?",
        };
        // Whole-pixel line heights on every text that sets a row's height
        // (the title, "asked", the folder, "allow all", the footer): the
        // text's own (1.5 × its size) is a half pixel at 15 or 13, and a
        // card a half pixel tall puts every row under it, and the
        // following list's end, off the pixel grid (CI's runners land an
        // f32 ulp off where bella's content summed exactly).
        let line = |size: f32| px((size * 1.5).round());
        let header = h_flex()
            .flex_none()
            .gap(px(8.))
            .items_center()
            .child(dot)
            .child(
                div()
                    .flex_none()
                    .font_family(fonts::SANS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_px(m.card_title)
                    .line_height(line(m.card_title))
                    .text_color(p.ink)
                    .debug_selector(|| "card-title".into())
                    .child(title),
            )
            .child(div().flex_1())
            .children(c.asked.clone().filter(|_| !self.panes.cur.layout.tight).map(|a| {
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(fonts::SANS)
                    .text_px(m.meta)
                    .line_height(line(m.meta))
                    .text_color(p.dim_t)
                    .debug_selector(|| "card-asked".into())
                    .child(a)
            }));
        let folder = c.guard.as_ref().map(|g| {
            mono(g.dir.clone(), m.meta, p.dim_t)
                .line_height(line(m.meta))
                .min_w_0()
                .flex_shrink(1.)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .debug_selector(|| "card-folder".into())
        });
        let scope = pr.allow_all().filter(|_| offered).cloned();
        let check_focused = live && c.box_focus && keys;
        let check = scope.map(|s| {
            let tip: SharedString = s.allow_all_scope().into();
            // "Don't ask again for guard this session" (§5); at 1280 the short label.
            let what: SharedString = s.allow_all_label().into();
            let text = if tight { SharedString::new_static("Don't ask again") } else { what };
            h_flex()
                .id("p-all")
                .debug_selector(|| "card-allow-all".into())
                .flex_none()
                .gap(px(8.))
                .items_center()
                .when(live, |d| d.track_focus(&self.panes.cur.allow_all_focus))
                .when(live && can, |d| d.cursor_pointer())
                .child(
                    div()
                        .flex_none()
                        .size(px(CHECK))
                        .rounded(px(3.))
                        .border_1()
                        // Focused (Tab, a click): Space toggles it.
                        .border_color(if check_focused { p.focus_k } else { p.dim_t })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(allow_all, |d| {
                            d.child(div().size(px(CHECK_IN)).rounded(px(1.)).bg(p.acc).debug_selector(|| "card-allow-all-on".into()))
                        }),
                )
                .child(
                    div()
                        .font_family(fonts::SANS)
                        .text_px(m.meta)
                        .line_height(line(m.meta))
                        .whitespace_nowrap()
                        .text_color(if allow_all { p.prose_t } else { p.dim_t })
                        .child(text),
                )
                .tooltip(move |window, cx| crate::overlay::tip(tip.clone(), window, cx))
                .when(live && can, |d| {
                    d.on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.panes.cur.allow_all_focus, cx);
                        this.toggle_allow_all(cx);
                    }))
                })
        });
        // What the card is waiting on: on its own line under the strip,
        // whole, never cut to a fragment beside the buttons.
        let foot = footer.map(|f| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_family(fonts::SANS)
                .text_px(m.meta)
                .line_height(line(m.meta))
                .text_color(if footer_warn { p.warn } else { p.dim_t })
                .debug_selector(|| "card-footer".into())
                .child(f)
        });
        let status = foot.map(|f| h_flex().flex_none().mt(px(8.)).items_center().child(f));
        // With split panes, each pane's card is named by its pane (tests).
        let pane = self.panes.split().then(|| self.panes.cur.id.index());
        // The folder and the box left, the buttons right; in a narrow card
        // the buttons wrap under them, still right.
        let line = h_flex()
            .id("prompt-actions")
            .flex_none()
            .flex_wrap()
            .mt(px((10. * r).round()))
            .min_h(px(crate::fit::button_h(m)))
            .gap_x(px(16.))
            .gap_y(px(8.))
            .items_center()
            .justify_between()
            .track_scroll(&c.actions)
            .child(h_flex().flex_1().min_w_0().gap(px(16.)).items_center().children(folder).children(check))
            .children(actions.map(|a| {
                a.flex_none().justify_end().debug_selector(move || match pane {
                    None => "card-buttons".into(),
                    Some(i) => format!("card-buttons {i}"),
                })
            }));
        v_flex()
            .id("prompt-card")
            .debug_selector(move || match (live, pane) {
                (true, None) => "card".into(),
                (true, Some(i)) => format!("card {i}"),
                (false, None) => "card-exit".into(),
                (false, Some(i)) => format!("card-exit {i}"),
            })
            .w_full()
            .relative()
            .rounded(px(crate::cards::Geo::of(m).radius))
            .border_1()
            .border_color(p.acc_line)
            .bg(card_bg)
            .px(px((14. * r).round()))
            .pt(px((12. * r).round()))
            .pb(px((12. * r).round()))
            // A click on a confirm card gives it the keys (a secret's
            // field takes them itself).
            .when(!secret && live, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        #[cfg(test)]
                        {
                            this.card_presses += 1;
                        }
                        if !this.focus.is_focused(window) {
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }
                    }),
                )
            })
            .child(header)
            .when(has_well, |d| {
                // A press on the bar gives a confirm card the keys, as one on the card does.
                let bar = crate::scrollbar::Bar::new(c.scroll.clone(), &c.bar);
                let bar = if secret || !live { bar } else { bar.focus(self.focus.clone()) };
                d.child(
                    div()
                        .mt(px((10. * r).round()))
                        .relative()
                        .rounded(px(crate::cards::Geo::of(m).well_radius))
                        .overflow_hidden()
                        .border_1()
                        .border_color(p.line)
                        .child(well)
                        .child(bar),
                )
            })
            .children(status)
            .child(line)
    }
}

/// Allow's style (Haseeb, 08:07: "make fill vs outline trivially
/// switchable"): the accent filled (the default, cards4/proposal.md §5),
/// or `AllowStyle::Outline`, the accent's label in its `acc_line` edge on
/// the card. Both paint and are tested (`cards::tests`, and the card under
/// each in `ui_tests`). [`allow_style`] is what a frame draws: this, or
/// `SYNAPS_DESK_ALLOW=outline` for a look at the other side (evidence).
pub(crate) const ALLOW_STYLE: crate::cards::AllowStyle = crate::cards::AllowStyle::Fill;

#[cfg(test)]
thread_local! {
    /// A test's Allow style (none: [`ALLOW_STYLE`]).
    pub(crate) static TEST_ALLOW: std::cell::Cell<Option<crate::cards::AllowStyle>> = const { std::cell::Cell::new(None) };
    /// Each receipt drawn, in order: its row, its word, whether it was lit
    /// (#25).
    pub(crate) static RECEIPT_LIT: std::cell::RefCell<Vec<(u64, &'static str, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The Allow style this frame draws ([`ALLOW_STYLE`]).
pub(crate) fn allow_style() -> crate::cards::AllowStyle {
    #[cfg(test)]
    if let Some(s) = TEST_ALLOW.with(|c| c.get()) {
        return s;
    }
    static ENV: std::sync::LazyLock<Option<crate::cards::AllowStyle>> =
        std::sync::LazyLock::new(|| match std::env::var("SYNAPS_DESK_ALLOW").ok().as_deref() {
            Some("outline") => Some(crate::cards::AllowStyle::Outline),
            Some("fill") => Some(crate::cards::AllowStyle::Fill),
            _ => None,
        });
    ENV.unwrap_or(ALLOW_STYLE)
}

/// Where the top edge's dashes start, `DASH` on and about `DASH_GAP` off,
/// the gaps spread so the run starts and ends on a dash at both corners.
// Q2's, kept while its tests stand: Cards D retires it (cards4/proposal.md §5, C2, C13).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn dash_xs(w: f32) -> Vec<f32> {
    if w < DASH {
        return Vec::new();
    }
    let n = ((w + DASH_GAP) / (DASH + DASH_GAP)).floor().max(1.) as usize;
    if n == 1 {
        return vec![((w - DASH) / 2.).round()];
    }
    let step = (w - DASH) / (n - 1) as f32;
    (0..n).map(|i| (i as f32 * step).round()).collect()
}

/// The dashes the trace's top stroke has passed, its head at `head`: the
/// whole ones, and the one under the head cut there.
// Q2's, kept while its tests stand: Cards D retires it (cards4/proposal.md §5, C2, C13).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn dash_spans(w: f32, head: f32) -> Vec<(f32, f32)> {
    dash_xs(w).into_iter().map(|x| (x, (head - x).clamp(0., DASH))).filter(|&(_, dw)| dw > 0.).collect()
}

/// What of a well `top..bottom` (window px) is unseen: its own overflow
/// still below (`rest`), and what lies outside the list's view `top..view`,
/// above it (Shady M3: in the log the card's top can scroll away) or under it.
pub(crate) fn unseen((well_top, well_bottom): (f32, f32), rest: f32, (top, view): (f32, f32)) -> f32 {
    hidden(rest, well_bottom, view) + (top - well_top).max(0.)
}

/// The card in its row: 8·r above it, 12·r below (polish.md §2.6), as
/// padding, so the row's measure is all of it.
fn card_slot(card: Stateful<Div>, m: &Metrics) -> Div {
    div().w_full().pt(px((8. * m.r).round())).pb(px((12. * m.r).round())).child(card)
}

/// A receipt row's height: one mono meta line with room round it.
pub(crate) fn receipt_line(m: &Metrics) -> f32 {
    m.row
}

#[cfg(test)]
mod tests {
    use super::{Answer, PromptRow, Receipt, applied, asked, call_ix, dash_xs, guard_shape, run_start, status_beside, took, with_prompts};
    use crate::turns::{Disp, Gap, Kind, Who};
    use std::time::{Duration, Instant};

    const GUARD: &str = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/work/relay-gateway:\n$ cargo test -p relay --test refresh_race\n\nAllow this tool call?";

    /// A confirm as the daemon sends it: `body`, and a scope (`None`: none).
    fn confirm(body: &str, scope: Option<serde_json::Value>) -> synaps_client::protocol::PromptRequest {
        let mut v = serde_json::json!({"id": 1, "kind": "confirm", "title": "Confirm tool call", "prompt": body});
        if let Some(s) = scope {
            v["confirm_scope"] = s;
        }
        serde_json::from_value(v).unwrap()
    }

    fn guard_scope() -> Option<serde_json::Value> {
        Some(serde_json::json!({"extensions": ["guard"], "tool": "bash"}))
    }

    #[test]
    fn a_guard_confirm_shows_its_command_and_folder() {
        let g = guard_shape(&confirm(GUARD, guard_scope())).expect("a guard's shape");
        assert_eq!((g.command.as_ref(), g.dir.as_ref()), ("cargo test -p relay --test refresh_race", "~/work/relay-gateway"));
        assert_eq!(guard_shape(&confirm(&GUARD.replace("\n", "\r\n"), guard_scope())), Some(g), "CRLF reads the same");
        // Anything else is the whole body, as today.
        assert_eq!(guard_shape(&confirm("Tool: bash\n\nInput that will run:\n$ ls\n\nAllow this tool call?", guard_scope())), None);
        assert_eq!(
            guard_shape(&confirm("Extension: guard\n\nMessage from guard:\nedit writes outside the folder:\n{}", guard_scope())),
            None
        );
        assert_eq!(guard_shape(&confirm("Activate shell_start?", guard_scope())), None);
    }

    /// Shady H1: the guard's shape only where the daemon framed the prompt
    /// cleanly and nothing rewrote what runs; its message whole.
    #[test]
    fn a_rewritten_or_unscoped_confirm_is_shown_whole() {
        assert_eq!(guard_shape(&confirm(GUARD, None)), None, "no scope: who asks is unknown");
        let rewritten = "Extension: guard\nTool: bash\nInput rewritten by: wrap\nInput that will run:\n{\"command\": \"wrap cargo test\"}\n\
                         Message from guard:\nbash wants to run a command in ~/w:\n$ cargo test\n\nAllow this tool call?";
        let scope = |by: &[&str]| Some(serde_json::json!({"extensions": ["guard"], "tool": "bash", "rewritten_by": by}));
        assert_eq!(guard_shape(&confirm(rewritten, scope(&["wrap"]))), None, "rewritten: the well shows what runs");
        assert_eq!(guard_shape(&confirm(rewritten, scope(&[]))), None, "the daemon's header holds more than its two lines");
        let spoof = "Message from guard:\nbash wants to run a command in ~/w:\n$ ls\n\nAllow this tool call?";
        assert_eq!(guard_shape(&confirm(spoof, guard_scope())), None, "no daemon header above the message");
        let many = "Extension: guard\nTool: bash\n\nMessage from guard:\nbash wants to run a command in ~/w:\n$ make a \\\n  && make b\nwarning: touches /etc\n\nAllow this tool call?";
        let g = guard_shape(&confirm(many, guard_scope())).expect("a guard's shape");
        assert_eq!(g.command.as_ref(), "make a \\\n  && make b\nwarning: touches /etc", "every line of the message after its intro");
    }

    /// Shady H2: a receipt names its call by the daemon's tool-use id, so a
    /// fresh fold (a reopened tab, a cache miss) that numbers the items
    /// anew still finds it, and a compaction that dropped it finds nothing.
    #[test]
    fn a_receipt_finds_its_call_after_a_refold() {
        use synaps_client::protocol::{LlmEvent, SessionEvent, StreamEvent};
        let mut live = synaps_client::transcript::Transcript::new();
        live.apply(&SessionEvent::Stream { event: StreamEvent::Llm(LlmEvent::ResponseStart) });
        for id in ["toolu_a", "toolu_b", "toolu_c"] {
            live.apply(&SessionEvent::Stream {
                event: StreamEvent::Llm(LlmEvent::ToolUse {
                    tool_name: "bash".into(),
                    tool_id: id.into(),
                    input: serde_json::json!({"command": id}),
                }),
            });
        }
        let call = |id: &str| serde_json::json!({"type": "tool_use", "id": id, "name": "bash", "input": {"command": id}});
        let result = |id: &str| serde_json::json!({"type": "tool_result", "tool_use_id": id, "content": "ok"});
        let mut fresh = synaps_client::transcript::Transcript::new();
        fresh.apply_history(&[
            serde_json::json!({"role": "user", "content": "first"}),
            serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "on it"}, call("toolu_a"), call("toolu_b"), call("toolu_c")]}),
            serde_json::json!({"role": "user", "content": [result("toolu_a"), result("toolu_b"), result("toolu_c")]}),
        ]);
        let id_of = |t: &synaps_client::transcript::Transcript, c: &str| call_ix(t.items(), c).map(|ix| t.items()[ix].id);
        assert_ne!(id_of(&live, "toolu_b"), id_of(&fresh, "toolu_b"), "the fold numbers its items anew");
        for t in [&live, &fresh] {
            let ix = call_ix(t.items(), "toolu_b").expect("found by its tool-use id");
            assert_eq!(t.items()[ix].tool.as_ref().unwrap().tool_id, "toolu_b");
        }
        let mut compacted = synaps_client::transcript::Transcript::new();
        compacted.apply_history(&[
            serde_json::json!({"role": "user", "content": "summary of what came before"}),
            serde_json::json!({"role": "assistant", "content": [call("toolu_c")]}),
        ]);
        assert_eq!(call_ix(compacted.items(), "toolu_a"), None, "compacted away: no receipt line, never a wrong one");
        assert!(call_ix(compacted.items(), "toolu_c").is_some());
    }

    /// The finish gate of #18 (optional, taken): the dashes start and end on
    /// a dash at both corners, at any width.
    #[test]
    fn the_dashes_end_on_both_corners() {
        for w in [100., 591., 592., 600., 624., 672., 700., 1234.5] {
            let xs = dash_xs(w);
            assert_eq!(xs[0], 0., "{w}");
            assert!((xs.last().unwrap() + super::DASH - w).abs() <= 0.5, "{w}: {xs:?}");
            for p in xs.windows(2) {
                let gap = p[1] - p[0] - super::DASH;
                assert!((super::DASH_GAP - 1.5..=super::DASH_GAP + 3.).contains(&gap), "{w}: gap {gap}");
            }
        }
        assert!(dash_xs(4.).is_empty());
    }

    /// Shady M3: the read gate counts the well's top edge against the list's
    /// view, not only its bottom against the window's.
    #[test]
    fn a_well_scrolled_above_the_list_is_not_read() {
        use super::unseen;
        use crate::app::authorize_ready;
        let view = (120., 640.);
        assert_eq!(unseen((200., 300.), 0., view), 0., "whole in the view");
        assert_eq!(unseen((80., 300.), 0., view), 40., "40 px above the list's top");
        assert_eq!(unseen((500., 700.), 0., view), 60., "60 px under its bottom");
        assert_eq!(unseen((200., 300.), 25., view), 25., "its own overflow");
        assert!(!authorize_ready(true, Some(unseen((80., 300.), 0., view)), false), "its top unseen: not ready");
        assert!(authorize_ready(true, Some(unseen((80., 300.), 0., view)), true), "once read whole: ready");
        assert!(authorize_ready(true, Some(unseen((121., 300.), 0., view)), false));
    }

    /// The finish gate of #18: the dashes don't land all at once when the
    /// trace closes; they follow its top stroke's head.
    #[test]
    fn the_dashes_follow_the_trace() {
        use super::{DASH, dash_spans};
        let w = 624.;
        let all = dash_xs(w).len();
        assert!(dash_spans(w, -1.).is_empty(), "nothing before the head moves");
        let half = dash_spans(w, w / 2.);
        assert!(half.len() > all / 2 - 2 && half.len() < all / 2 + 2, "half the edge: {} of {all}", half.len());
        assert!(half.iter().all(|&(x, dw)| x + dw <= w / 2. + 0.01), "none past the head");
        assert!(half[..half.len() - 1].iter().all(|&(_, dw)| dw == DASH), "the ones behind it whole");
        assert_eq!(dash_spans(w, f32::INFINITY).len(), all);
        assert!(dash_spans(w, f32::INFINITY).iter().all(|&(_, dw)| dw == DASH));
    }

    #[test]
    fn the_header_says_what_asked_and_when() {
        let pr = |v: serde_json::Value| serde_json::from_value::<synaps_client::protocol::PromptRequest>(v).unwrap();
        let at = "2026-11-17T14:21:05Z";
        let local = crate::times::Stamp::local(at).unwrap().hhmm;
        let full = pr(serde_json::json!({"id": 1, "raised_at": at, "confirm_scope": {"extensions": ["guard"], "tool": "bash"}}));
        assert_eq!(asked(&full).unwrap(), format!("bash · guard · asked {local}"));
        let no_tool = pr(serde_json::json!({"id": 1, "raised_at": at, "confirm_scope": {"extensions": ["guard", "lint"]}}));
        assert_eq!(asked(&no_tool).unwrap(), format!("guard, lint · asked {local}"));
        assert_eq!(asked(&pr(serde_json::json!({"id": 1, "raised_at": at}))).unwrap(), format!("asked {local}"));
        assert_eq!(
            asked(&pr(serde_json::json!({"id": 1, "confirm_scope": {"extensions": ["guard"], "tool": "bash"}}))).unwrap(),
            "bash · guard"
        );
        assert_eq!(asked(&pr(serde_json::json!({"id": 1}))), None, "nothing known: nothing said");
    }

    #[test]
    fn a_receipt_says_what_and_when() {
        let r = |answer, took| Receipt {
            call: "toolu_1".into(),
            own: false,
            row: 1,
            secret: false,
            answer,
            at: "14:21".into(),
            took,
            landed: Instant::now(),
        };
        let words = |r: Receipt| {
            let (w, rest) = r.words();
            format!("{w}{rest}")
        };
        assert_eq!(words(r(Answer::Allowed, Some(Duration::from_secs(7)))), "allowed 14:21 · 7s");
        assert_eq!(words(r(Answer::Denied, None)), "denied 14:21");
        let running = r(Answer::Allowed, Some(Duration::from_secs(15)));
        let w = running.words_while(true);
        assert_eq!(format!("{}{}", w.0, w.1), "allowed 14:21", "running: the clock counts, not the wait");
        let t0 = running.landed - Duration::from_secs(15);
        assert_eq!(run_start(Some(t0), Some(&running)), Some(running.landed), "a gated call runs from its answer");
        assert_eq!(run_start(Some(t0), None), Some(t0));
        assert_eq!(words(r(Answer::Elsewhere, Some(Duration::from_secs(3)))), "answered elsewhere 14:21", "not ours: no wait");
        assert_eq!(words(r(Answer::Provided, None)), "provided 14:21");
        assert_eq!(words(r(Answer::Cancelled, Some(Duration::from_secs(4)))), "cancelled 14:21 · 4s");
        assert_eq!(words(r(Answer::Resolved, Some(Duration::from_secs(4)))), "resolved 14:21", "not ours: no wait");
        assert_eq!(status_beside(Some("denied"), Some(Answer::Denied)), None, "one denied, the receipt's");
        assert_eq!(status_beside(Some("denied"), None), Some("denied"));
        assert_eq!(status_beside(Some("cancelled"), Some(Answer::Cancelled)), None, "a HALT under the card: one cancelled");
        assert_eq!(status_beside(Some("cancelled"), Some(Answer::Elsewhere)), Some("cancelled"));
        // Shady L4: not ours, and its call cancelled: a cancel, said once.
        use synaps_client::transcript::{ToolCall, ToolStatus};
        let cancelled = Some(ToolStatus::Cancelled);
        assert_eq!(r(Answer::Elsewhere, None).answer_for(cancelled, None), Answer::Cancelled, "a HALT from another client");
        assert_eq!(r(Answer::Resolved, None).answer_for(cancelled, Some(false)), Answer::Cancelled);
        assert_eq!(r(Answer::Elsewhere, None).answer_for(Some(ToolStatus::Done), Some(true)), Answer::Elsewhere);
        assert_eq!(r(Answer::Elsewhere, None).answer_for(None, None), Answer::Elsewhere, "still running");
        // #25: a yes from here whose call shows it was held at its gate
        // says so; one that got past, or isn't known yet, stays ours.
        for a in [Answer::Allowed, Answer::AllowedAll, Answer::Provided] {
            assert_eq!(r(a, None).answer_for(Some(ToolStatus::Denied), Some(false)), Answer::Denied);
            assert_eq!(r(a, None).answer_for(cancelled, Some(false)), Answer::Cancelled);
            assert_eq!(r(a, None).answer_for(cancelled, Some(true)), a, "it ran: a cancel after it doesn't demote it");
            assert_eq!(r(a, None).answer_for(Some(ToolStatus::Running), None), a, "not known yet: what was sent");
        }
        assert_eq!(r(Answer::Denied, None).answer_for(Some(ToolStatus::Denied), Some(false)), Answer::Denied);
        assert_eq!(r(Answer::Denied, None).answer_for(cancelled, Some(false)), Answer::Denied, "a no stays a no");
        assert_eq!(status_beside(Some("cancelled"), Some(r(Answer::Elsewhere, None).answer_for(cancelled, None))), None);
        // [`applied`]: the engine's whole forms only, and what the call showed
        // before (`past`) is never taken back.
        let call = |status, result: &str| ToolCall { status: Some(status), result: result.into(), ..Default::default() };
        let gate = "Tool call blocked by extension: Tool call confirmation denied: guard wants to check this";
        let started = "Canceled by user. It had already started and may have partially applied its effects.";
        let partial = "running 6 tests\n\n[Canceled by user before the tool finished; the output above is partial.]";
        use ToolStatus::*;
        for (c, past, want, why) in [
            (call(Denied, gate), false, Some(false), "held at the gate"),
            (call(Cancelled, "Canceled by user"), false, Some(false), "a bare cancel, nothing streamed"),
            (call(Cancelled, ""), false, Some(false), "the turn ended before it reported, nothing streamed"),
            (call(Cancelled, started), false, Some(true), "the engine says it had started"),
            (call(Cancelled, partial), false, Some(true), "its partial output, the engine's note under it"),
            (call(Cancelled, "Canceled by user"), true, Some(true), "C64-1: it streamed before the HALT's bare cancel"),
            (call(Cancelled, "running 6 tests\n"), true, Some(true), "C64-1: settled after its output, no result"),
            (call(Denied, gate), true, Some(true), "C64-2: printed the gate's words after streaming"),
            (call(Running, ""), false, None, "silent: not known"),
            (call(Done, "ok"), false, Some(true), "ended on its own"),
            (call(Failed, "Tool execution failed: Command failed (exit 1):"), false, Some(true), "ended on its own"),
            // C64-2: a deny quoted inside output, or another block, is not the gate's.
            (call(Done, &format!("log:\n{gate}\n")), false, Some(true), "quoted mid-output"),
            (call(Denied, "Tool call blocked by extension: policy says no"), false, None, "another extension's block"),
            (call(Cancelled, "Canceled by user\nthen more"), false, None, "not the bare phrase"),
        ] {
            assert_eq!(applied(Some(&c), past), want, "{why}: {c:?}");
        }
        assert_eq!(applied(None, false), None);
        assert_eq!(applied(None, true), Some(true));
        assert_eq!(status_beside(Some("failed"), Some(Answer::Allowed)), Some("failed"));
        assert_eq!(status_beside(Some("waiting"), Some(Answer::Allowed)), Some("waiting"), "a secret still to come");
        assert_eq!((took(Duration::from_secs(125)), took(Duration::from_secs(3780))), ("2m 05s".into(), "1h 03m".into()));
    }

    #[test]
    fn a_prompt_row_follows_its_item() {
        let row = |item, id, kind| Disp { item, id, rev: 0, kind, gap: Gap::None, who: Who::None };
        let out =
            vec![row(0, 1, Kind::User), row(1, 2, Kind::Summary { end: 4, open: false }), row(4, 5, Kind::Work), row(5, 6, Kind::Prose)];
        let p = |after| PromptRow { after, id: 99 + after as u64, rev: 0 };
        let ids = |v: Vec<Disp>| v.iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(with_prompts(out.clone(), &[p(4)], 0)), [1, 2, 5, 103, 6], "after its call");
        assert_eq!(ids(with_prompts(out.clone(), &[p(2)], 0)), [1, 2, 101, 5, 6], "in a folded run: after the run");
        assert_eq!(ids(with_prompts(out.clone(), &[p(5)], 0)), [1, 2, 5, 6, 104], "at the end");
        assert_eq!(ids(with_prompts(out[2..].to_vec(), &[p(2), p(5)], 4)), [5, 6, 104], "a rebuild from 4 adds only its own");
    }
}
