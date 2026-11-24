# synaps-client changelog

Commit hashes are this repository's (`synaps-desk`). PR numbers other than synaps-desk#N are those of the archived `synaps-desktop` repository this history was rewritten from.

## Since d6f8f68

No change. #75 (`3f61cff`) added a mirror of the core card protocol and model (agent-ui D1, SynapsCLI #444 at `6e895d27`), inert; D0 removes it whole, because that core stack is closed and no daemon will speak it. Gone again: `FEATURE_AGENT_SURFACES`, `SURFACES_CATALOG`, `Capabilities::surfaces`, `SessionCommand::SurfaceAction` and `SurfaceAction`, the `surface` and `surface_result` events, `SessionMeta::{surfaces_waiting, surfaces_open}`, `Attached::surfaces`, `Rejected::surfaces`, `Client::surface_action_on` / `SessionRef::surface_action_on` / `Command::SurfaceActionOn`, and the `surface` module with `tests/fixtures/ui/`. The `Hello` was byte for byte the same throughout. Agent cards come back as a Synaps extension (`synaps-cards`: the `cards` extension and the `cards-spec` crate, design v1.2 §9), with no protocol change here.

## Since c89cc05

Saved sessions as catalog pages (SynapsCLI watch-sessions PR 4, `session_page`), the replacement of #408's `saved_sessions`. Additive apart from one new `SessionMeta` field (below); `saved_sessions` / `SavedSessionList` stay for daemons that still speak them.

- New `FEATURE_SESSION_PAGE` (`"session_page"`) and `CATALOG_PAGE_MAX` (200, the daemon's ceiling).
- New `ClientFrame::CatalogPage { req, before, limit, named_only }`, encoded as PR 4 pins it (`{"type":"catalog_page","req":7,"before":{…},"limit":50}`; `before` and a false `named_only` omitted), and `protocol::CatalogCursor(serde_json::Value)` (opaque, `serde(transparent)`: whatever JSON a reply's `next` is goes back as `before` unchanged, so a change of the cursor's shape never makes a page undecodable).
- New `DaemonFrame::CatalogPage { req, sessions, next, rejected, error }`. Rows decode one by one: an undecodable row is skipped and counted in `rejected`, the page is kept. A missing or `null` `sessions` is empty.
- New `Client::catalog_page(before, limit, named_only) -> Result<u64, RequestError>`: returns the `req`; `limit` clamped to 1..=200. Only to a daemon that lists `session_page` (else `Unsupported`, or `NotConnected` while features are unknown; nothing is sent). New `Command::CatalogPage { req, .. }` (a raw one to a daemon without the feature fails its `req` and says `Error::Unsupported`).
- New `Event::CatalogPage { req, sessions, next, rejected }` and `Event::CatalogPageFailed { req, reason }` (the reply's `error`, a dropped connection, not connected). Replies are matched by `req`, not order (the daemon serves pages as tasks); a reply to a `req` not outstanding is dropped. A `catalog_page` reply that does not decode (`DaemonFrame::Malformed { kind: "catalog_page" }`) fails the oldest page waiting, besides the `Event::Error { Decode }`. A control `Error` (no `req`) fails a page only if it names `catalog_page`, or starts with `malformed frame` while only pages wait; any other leaves pages pending.
- New `SessionMeta::updated_at: Option<String>` (set on saved rows; held rows omit it), `SessionMeta::is_saved()` (`lifecycle == "saved"`), and `SessionMeta::to_saved()` (the row as a `SavedSession`, journal id as `id`, `live: false`).
- **Breaking** for code that builds `SessionMeta { .. }` without `..`: add `updated_at`.
- `conn::Conn`: an `Attach` sent on a connection cancels the `CatalogPage` reqs outstanding on it (`Conn::cancelled()`); a late reply to one (zero or one may come before `Attached`) is dropped by `Conn::recv`. `Client` itself sends pages only on its control connection, which never attaches.

## Since 461a006

Additive only: workers in session lists from a `dev` daemon (SynapsCLI #416, which replaces #410's `subagents_running`).

- New `SessionMeta.workers: SessionWorkers` (`#[non_exhaustive]`, `running` only for now), read leniently: missing, `null` or a shape this client can't read is empty, and the session stays listed.
- New `SessionMeta::workers_running()`: `workers.running` or `subagents_running`, whichever the daemon sends. The desktop now reads the count through it.
- **Breaking** for code that builds `SessionMeta { .. }` without `..`: add `workers`.
- Tests: `tests/fixtures.rs` `session_meta_reads_workers_from_either_daemon`.

## Since 9b0d263

Engine events (synaps-desk `feat/events`). Additive; one behaviour change for history rows that are events.

- New `events` module: `ExternalEvent` (with `EventSource`, `EventChannel`, `EventSender`, `EventContent`, `Severity`), the engine's `events::types::Event` mirrored leniently (`id`, `source` and `content` required, every other field optional, unknown fields ignored; `severity` any case, anything else `Medium`; `disclosure` kept as text). `ExternalEvent::from_payload` parses the history form (`format_event_for_agent`: `<event id type severity source [channel]>text[\nData: json]</event>`), strictly (a forged closer or opener, a missing attribute: `None`); `ExternalEvent::shown` puts a live event in the form its history row will have (the same text; a withheld disclosure class as the same marker, its data dropped); `is_payload`, `is_worker_note`, `withheld`, `severity`.
- New `SessionEvent::External { event: Box<ExternalEvent> }` (it decoded to `Unknown` before; a body without `id`, `source` or `content` is now a `DecodeError`).
- New `ItemKind::Event` and `Item::event: Option<Arc<ExternalEvent>>`. **Behaviour:** a history user message that is an event's form (not a worker's note) is an `Event` row, not a `Notice` of the raw text; a live `External` (or a `Steered` / `SteeringDelivered` carrying an event's form) adds one, a note until its history message lands, then that message's row (same id, its live time and fields kept). One row per event id; never a `User` row, never a `steered:` notice. Worker notes (`source = subagent`, `subagent_completion` / `_steer` / `_cancelled`) are `Notice`s as before and add no live row. A `<event` message that does not parse stays a `Notice`.
- Tests: `events::tests` (decode, the `format.rs` vectors, forged forms, disclosure, a 3000-case fuzz) and `tests/events.rs` (wake and mid-turn parity, dedupe, worker notes).
- Round 2 (Shady #22):
  - `ExternalEvent::form` (the agent form, out of `cfg(test)`) and `::history` (it, parsed). `::shown` is now the form read back by `from_payload`'s parser plus the frame-only fields, so a live row's text, type, severity, source, channel name and data are its history row's by construction. The data line is only the last line, one line, `Data: {`/`[`, JSON or the 1000-character cut, and never on a live row whose event has no data. `local_only` reads as its text in `shown` (`::local_only` says so); its form keeps the marker. The sender's fields are attribute values (no bidi, invisible or control characters). `EventContent`'s `Debug` gives sizes, not words.
  - **Behaviour:** a live event row is held as the last row until the engine delivers it (`steering_delivered` with its form, a turn's start, end or abort, or its message in a history append); rows streamed meanwhile go above it, and `response_reset` keeps it. A `Steered` frame is always the owner's `steered:` notice. A history user message is an event form only if `is_payload` (a user's `<event loop>…` is a `User` row). A frame is the same event as an existing row only with the same id and the same time, or (a history row) the same form; another event with a reused id is another row. A refold (re-attach, replacement) keeps an event row's frame-only fields.
  - Tests: `tests/events.rs` mid text stream, mid thinking then a tool round, held through a turn with mid-turn appends, a reset during the hold, `never_persist`, reused ids, re-attach and replacement, L1, L2; `events::tests` data lines, `local_only`, sender sanitising, `Debug`.
- Round 3 (Shady #22 re-review):
  - `ExternalEvent::history` and `::shown` read the form with `from_payload` (no data hint): a text whose last line is a JSON `Data:` line reads as text plus that data everywhere, live and history alike (one identity). The 1000-character cut is text (no longer dropped). New `ExternalEvent::data_text` (`Data: {json}`, the line the agent reads).
  - New `pending_events_len` on `ConversationDigest` and `ConversationSnapshot` (`serde(default)`; sent since v0.10.0).
  - **Behaviour:** after a failed turn (a stream `error`, or a stream that ends with no end), a failed or cancelled compaction, or an attach with no turn running while the engine holds events, the held event rows stay held (the engine flushes them at the next turn's end) until their message lands, `steering_delivered`, or a digest with `pending_events_len == 0`. `Cleared` drops held event rows. An attach with no turn running delivers the rest (a `never_persist` row is placed). A history row matches a frame by form only during an attach's replay (outside one, a frame after a row with its form is another event). Held rows match their messages by exact form first, then by id (never a `never_persist` row).
- Round 4 (Shady #22 round 3):
  - `from_payload`: a last `Data:` line is data only if it is `serde_json::to_string` of the JSON it parses to, character for character, and at most 1000 characters; any other (a longer line, duplicate keys, other spacing, an escape such as `\u202e`) is a line of the text, as written. So `data_text` of a shown or history event is the line the agent reads, exactly. The body and the attribute values are read through the form's sanitiser (a no-op on an engine-written form).
  - **Behaviour:** `Resumed` drops held event rows, as `Cleared` does.

For synaps-avatar (nyx): nothing breaks (`ItemKind` and `SessionEvent` are `#[non_exhaustive]`); history events now come as `ItemKind::Event` rather than `Notice`.

## Since c9dc8c7

Additive only (synaps-desk#11 `feat/favourites`): the daemon's word on a resume that followed a compaction chain. Nothing existing changes shape.

- New `SessionMeta::continue_info: Option<ContinueInfo>`, decoded leniently as `roster` is: `null`, missing or a shape this client doesn't read (not an object, a field of another type) is `None`, and never fails the attach; and `protocol::ContinueInfo` (`#[non_exhaustive]`, every field `serde(default)`), mirroring v0.11.1's `ContinueInfoWire`: `session_id` (the journal opened), `resolved_via` (`"compacted"`, `"chain"`, `"name"` or none), `query` (what was asked for), `compaction_notice`. Set at create, so the `Attached` snapshot's meta (the create-time copy) carries it. `ContinueInfo::compacted()`: the daemon followed `query`'s compactions to `session_id` (F24). A `SessionMeta` struct literal needs `..Default::default()` as before.
- Fixture fix: `tests/fixtures/session_swaps.ndjson`'s conversation after `compaction_applied` now names its parent (`parent_session`: the previous journal), as the engine's `Session::from_compaction_record` sets it; it had `null`.
- Test: `tests/fixtures.rs` `session_meta_carries_continue_info` (with malformed values: `None`, the meta still read).

For synaps-avatar (nyx): nothing to change.

## Since 8e69eda

Message times (synaps-desk#12 `feat/message-times`). **One breaking change**, `Event::Session`'s new `ts` field; the rest is additive.

- **Breaking:** `Event::Session` has a new field, `ts: String`: its envelope's `ts` (RFC 3339, when the daemon sent it; empty if it had none). A struct literal without it no longer compiles (E0063), nor does a pattern that names every field without `..` (E0027). The variant is now `#[non_exhaustive]`, so this is the last time a field breaks it: outside this crate it can only be built with the new `Event::session(attach_seq, session_id, seq, event, raw)` (sent at no known time, `ts` empty), plus `.with_ts(ts)` to give it a time, and only matched with `..`.
- New `Item::at: Option<Arc<str>>`: when the row happened, RFC 3339. A live row has the `ts` of the event that started it, and keeps it when its history form lands, as the TUI's live rows: the daemon dates a message when its round ends, which can be minutes later. A row first seen in history (an attach, a resume, a rebuild) has its message's `_synaps_at`. A legacy message (no `_synaps_at`) and a row fed without a time have `None`, never "now". After a conversation swap (clear, resume, compaction) no row takes a time from the row it replaces, also on a re-attach after a swap made while detached: `Transcript` now remembers the journal id (`SessionMeta::journal_id`, moved on by `Cleared`, `Resumed` and `CompactionApplied`) and carries times only to the same journal. A history append always keeps the first times. `at` is not part of `rev`: it can change with `rev` unchanged (a live row without a time, then its history form; a swapped conversation's row), so key anything derived from it on `at` itself. `Item` was already unbuildable outside this crate (a private field), so this is additive.
- New `transcript::MESSAGE_AT_KEY` (`"_synaps_at"`, agent-core `session_journal::MESSAGE_AT_KEY`) and `transcript::message_at(&Value) -> Option<&str>`.
- New `Transcript::apply_at(&SessionEvent, ts)` and `Transcript::apply_watch_at(&WatchEvent, at)`: `apply` / `apply_watch` whose new rows carry `ts` / `at`. `apply` and `apply_watch` are unchanged (their rows have no live time).
- Behaviour: `Transcript::load_attached` replays the attach's in-flight events with their envelopes' `ts` (`apply_at`), so a row the replay starts has a time.
- Tests: `tests/fixtures.rs` `rows_carry_their_message_times` (the captured v0.11.1 turn: the thinking row streamed from 07:34:28 keeps that time although its history says 07:34:30; a re-attach keeps it, a fresh attach has the history's; a legacy row has none; a swapped conversation's same-text row at a reused id takes no time), `a_swap_with_no_replacement_keeps_live_times`, `a_reattach_after_a_clear_elsewhere_carries_no_time`.

For synaps-avatar (nyx): three test literals in `crates/synaps-avatar/src/session.rs` stop compiling: a struct literal of a `#[non_exhaustive]` variant is refused outside its crate (E0639), so adding `ts` to them is not enough; build them with the constructor. Production code (`:510`, `Event::Session { event, .. }`) compiles unchanged; its rows just carry no live time (avatar draws none).
- `:897` (the `turn()` helper, which has the envelope): `Event::session(attach_seq, session_id, seq, event, raw).with_ts(env.ts.clone())`.
- `:1057` (`prompts_need_you_until_resolved`) and `:1074` (`abort_is_annoyed_with_a_reason`): `Event::session(attach_seq, session_id, seq, event, raw)`.

To show times as the desktop does, apply events with `transcript.apply_at(&event, &ts)` (`Event::Session { event, ts, .. }`).

## Since ca644b1

Additive only (synaps-desk `feat/worker-tabs`): cancelling a worker, as the TUI's `/workers cancel sa_N`. Nothing existing changes shape (`Command`, `Event` and `SessionCommand` are `#[non_exhaustive]`); no default changes.

- New `Client::cancel_worker(handle_id)` and `Command::CancelWorker { handle_id }`: the engine command `{"cmd":"engine_command","id":…,"name":"workers","arg":"cancel sa_N"}` (v0.11.1 `actor_cmds.rs`), owner-only like a steer. The worker stops at its next step and sends no completion wake; the engine notes the cancel in the main conversation (an `external` event, `content_type` `subagent_cancelled`). One worker only: a handle that is not `sa_` and digits (`all`, which v0.11.1 reads as every running worker) is never sent and is answered `Err`.
- New `SessionCommand::EngineCommand { id, name, arg }` (the wire command it goes as).
- New `Event::WorkerCancel { attach_seq, handle_id, result }`: the reply, matched by its query id (another client's broadcast `query_result` never counts). `Ok(text)`: the daemon's `{"kind":"output"}` (`Cancelled sa_3 (they stop at their next step; …)`). `Err(why)`: its `{"kind":"error"}` (`sa_3 is already completed — nothing to cancel`), a non-owner's `refused` (`refused: input is owned by client #1`), a cancel in flight when the connection dropped, the session ended, the daemon said bye or the session was detached, one asked for while not connected, or a handle that is not one worker's. Every cancel gets exactly one answer.
- Tests: `tests/fake_daemon.rs` `cancel_worker_wire_shape_and_replies`, `cancel_worker_sends_one_handle_and_always_answers`.

For synaps-avatar (nyx): nothing to change.

## Since dc5a246

Additive only (synaps-desk#1 `feat/worker-tray`): watching and steering a session's workers (subagents), as the Synaps TUI does. Nothing existing changes shape: `Command`, `Event`, `Error`, `SessionCommand` and `SessionQuery` were already `#[non_exhaustive]`.

- New `Command::SubagentWatch { handle_id, since }` and `Client::watch_subagent(handle_id, since)`: one page of a worker's watch journal (`{"cmd":"query","query":{"query":"subagent_watch","handle_id":…,"since":…,"tool_queue":true}}`, Synaps v0.11.0+). The reply comes as the new `Event::SubagentWatch { attach_seq, session_id, handle_id, since, reply }`, `reply` a new `WatchReply` (`#[non_exhaustive]`): `Page(Box<WatchPage>)`, `Gone(String)` (the daemon's `{"error": …}`: an unknown worker, or one that finished longer ago than the daemon keeps a journal), `Unsupported` (a daemon before v0.11.0 could not decode the query), `Undecodable(String)` (with an `Error::Decode`).
- New `protocol::WatchPage` and `protocol::WatchEvent` (both `#[non_exhaustive]`), mirroring v0.11.1's `runtime/subagent_watch.rs`; every field `serde(default)`, and a kind this client does not know is `WatchEvent::Unknown`, never a failed page. `WatchEvent::to_stream()` gives the main transcript's shape.
- New `Command::SubagentSteer { handle_id, text }` and `Client::steer_subagent(handle_id, text)`: steer a running worker (`{"cmd":"subagent_steer",…}`, an input command: refused unless this client owns the session's input). Its `Debug` redacts the text, as `Submit`'s does. A refusal is `Error::Refused { command: Some("subagent_steer"), reason }`; from a daemon before v0.11.0 the reason is the new `STEER_UNSUPPORTED`.
- Behaviour, only for the two new commands: a daemon before v0.11.0 answers a frame it cannot decode with `malformed frame: unknown variant …` and keeps the connection; for `subagent_watch` / `subagent_steer` that now becomes `WatchReply::Unsupported` / the `STEER_UNSUPPORTED` refusal instead of an `Error::Protocol`.
- New `Transcript` methods for a transcript fed by hand (a worker's): `apply_watch(&WatchEvent)`, `set_busy(bool)` and `trim_live_front(max_items, max_bytes)` (drops the oldest rows of a transcript with no history; returns how many; the next splice starts at 0).
- Tests: `tests/fixtures.rs` (a v0.11.1 page decodes and replays like the main stream; the query and the steer encode as the daemon reads them, the text never printed; a live-only transcript trims its oldest rows) and `tests/fake_daemon.rs` (pages, `Gone`, an old daemon's `unknown variant`).

For synaps-avatar (nyx): nothing to change.

## Since 3efcca2

Additive only (PR `feat/rail-redesign`): the daemon's `SessionMeta` fields the rail reads, mirrored.

- New `SessionMeta::roster: Vec<ClientInfo>` (every attached client, named; `serde(default)`, a `null` or missing list is empty, an entry that does not decode is dropped and the session stays listed), `SessionMeta::park_due` and `SessionMeta::evict_due` (`Option<String>`, RFC 3339, as `state_since`). A struct literal needs `..Default::default()` (nothing in this workspace or in synaps-avatar builds one; both decode it).
- `SessionMeta` now derives `PartialEq`, and `ClientInfo` `PartialEq` and `Eq`, so a consumer can tell when a polled list changed.

For synaps-avatar (nyx): nothing to change.

## Since 0451a7a

Additive only (PR `polish/layout-backlog`), plus two behaviour fixes. Nothing existing changes shape: code that builds `Event::Attached { .. }` or matches `Command`, `ToolOutcome` or `ToolStatus` exhaustively (all `#[non_exhaustive]`) compiles as before.

- New `Attached::connection: u64` (not on the wire; `0` from serde, set by the client): this `Client`'s number for the connection the snapshot came on, new on every attach and every reconnect, never reused. `Attached` is `#[non_exhaustive]`, so no literal elsewhere builds it.
- New `Command::AnswerOn { connection, prompt_id, value }` and `Client::answer_on(connection, prompt_id, value)`: an `Answer` that is sent only on that connection. Once the session has reconnected (a reload, a restarted daemon that numbers its prompts from 1 again), it is dropped, never sent, and comes back as `Error::Refused { command: Some("answer"), .. }`; while reconnecting it is refused as every command is. `Client::answer` is unchanged (whatever connection is up when the thread gets to it). Its `Debug` redacts the value, as `Answer`'s does. Test: `tests/fake_daemon.rs` `answers_for_an_earlier_connection_are_never_sent`.
- Behaviour: `tool_outcome` calls an `activate_tools` error `Denied` only for the daemon's refusal, `tool activation requires host confirmation authority` (`HostActivationError::ConfirmationRequired`: the user denied the prompt, the policy is `deny`, or nothing could show one). Every other `tool activation denied: …` (a duplicate id, an already active tool, a digest or provenance error, a guessed id: failures after AUTHORIZE) is `Failed`.

- New `ToolOutcome::Unknown` and `ToolStatus::Unknown` (`is_terminal()` includes it). **Behaviour:** a result whose last line is the bash tool's output-limit notice is `Unknown`, not `Ok`: the daemon killed the command there and returns the result as a success anyway. Both forms: v0.11's `[output truncated at 262144]` and agent-engine `origin/dev`'s `output_limit_notice` (`[output truncated at N bytes: the command was stopped at this limit before it finished, so its exit status is unknown. …]`). Also when the notice ends the tail of a virtualised history result (`runtime/tool_spill.rs`: the tail, a blank line, then `[Output virtualised: …]`; the daemon's default above 12 KiB). Not detectable, so still `Ok`: the live `ToolResult` (cut to `max_tool_output`, 30 000 bytes by default, before the notice; the history that follows replaces it) and a history without virtualisation (also cut before the notice).
- The refusal of an `AnswerOn` for an earlier connection now says "the prompt was raised on an earlier connection; not sent" (true after another attach as well as after a reconnect).

For synaps-avatar (nyx): nothing to change. To make answers airtight, keep `attached.connection` from the last `Event::Attached` and answer with `client.answer_on(connection, id, value)`.

## Since 12d89d9

No API change. Two behaviour fixes in `transcript::tool_outcome` (PR `fix/review-pr5`):

- An `activate_tools` call whose confirmation was denied (`Tool execution failed: tool activation denied: …`, agent-engine `tools/discovery.rs` `activation_denied`) is `Denied`, not `Failed`. A guessed id (`… tool activation denied: unknown tool id …`) stays `Failed`.
- `Unknown tool` matches only the daemon's `Unknown tool: ` (with the colon), so a result that merely starts with the words is not a failure.

## API changes since 66ffc7a

Additive only (the hybrid UI, PR #5 `feat/hybrid-ui`), plus one behaviour fix (tool status from the result text).

- `ConversationSnapshot` gained `tokens: Value` (`serde(default)`): the attach snapshot's `ConversationTokens` (`{input, output, cache_read, cache_creation}`), which `ConversationDigest::tokens` already carried on every later `Conversation` event. `Null` from a daemon that omits it. A struct literal needs `..Default::default()` (nothing in this workspace builds one).
- `ConversationSnapshot` gained `keepwarm_tokens: Value` (`serde(default)`): the keep-warm ping tokens the daemon says are already included in `tokens`. `Null` from a daemon that predates them. (Not on `ConversationDigest`: that would grow every `Event::Session`; read it from the event's `raw` JSON, `digest.keepwarm_tokens`.)
- New `transcript::ToolOutcome { Ok, Failed, Denied, Cancelled }` (`#[non_exhaustive]`), `transcript::tool_outcome(text, is_error)` and `transcript::tool_result_outcome(&block)`: a finished tool call classified by its result text, as the Synaps TUI does. The engine leaves `is_error` unset on a failed tool (`Tool execution failed: …`), an extension block (a denied confirmation: `Tool call blocked by extension: …`), a gate denial (`Tool call denied: …`, `Unknown tool: …`) and a canceled call (`Canceled by user`, …), so `is_error` alone called all of them successes.
- New `ToolStatus::Denied` (`ToolStatus` is `#[non_exhaustive]`; `is_terminal()` includes it). **Behaviour:** `Transcript` now sets a tool row's status from `tool_outcome` (history and live `ToolResult`): those results were `Done`, now `Failed` / `Denied` / `Cancelled`.

## API changes since 526f307

Named and saved sessions (PR #4). Old → new, one line each. **Breaking** for struct literals and exhaustive matches: `Status::Connected`, `CreateConfig`, `SessionMeta`; and for anyone who built on this PR's first push (ebf3f73): `rename`, `saved_sessions`, `Event::SavedSessions`.

- **Breaking:** `Status::Connected { daemon_version, profile, pid }` → also `features: Vec<String>` (`Welcome.features`); match it with `..`.
- **Breaking:** `CreateConfig { model_override, cwd, name, auto_approve_confirms }` → also `continue_session: Option<Option<String>>` (absent when `None`; `Some(None)` goes out as `null`, a bare `--continue`). A struct literal needs `..Default::default()`.
- **Breaking:** `SessionMeta { .. }` → also `awaiting_input: usize`, `journal_id: String`, `locked_by: Option<String>` (all `serde(default)`), plus `is(id)` (id or journal id). A struct literal needs `..Default::default()`.
- **Breaking (vs ebf3f73):** `Client::rename(session_id, Option<&str>, force: bool)` → `Client::rename(session_id, Option<&str>, expect: Expect)`. Always compare-and-set: `Expect::Unnamed` sends `expect: null`, `Expect::Named(n)` sends `expect: "n"`. The old `force` (no `expect`, replace any name) is gone: replace with `Expect::Named(<the current_name the user saw>)`, so a name changed meanwhile is refused again, not overwritten. Clearing (`None`) is compare-and-set too.
- **Breaking (vs ebf3f73):** `Client::saved_sessions(named_only, limit: usize)` → `limit: u32`, clamped to `1..=SAVED_SESSIONS_MAX` (1000; the daemon's field is `Option<u32>`). `Command::SavedSessions` / `ClientFrame::SavedSessions` `limit` → `u32`.
- **Breaking (vs ebf3f73):** `Event::SavedSessions { sessions, truncated }` → also `rejected: usize` (entries that did not decode; was an `Event::Error { Decode }`). A failed list (the daemon's `Error`, a dropped connection, not connected) was `Event::Error { Daemon | Transport | Refused }` → new `Event::SavedSessionsFailed { reason }`. A daemon without the feature still answers `Event::Error { error: Error::Unsupported { feature } }`.
- New `Client::resume(id_or_name, CreateOptions) -> Result<u64, ClientClosed>`: `Attach::Create` with `continue_session: Some(Some(id_or_name))` (every v3 daemon). Failures ("no session matching 'x'", a journal live in another process) arrive as `Event::Detached { reason }`. Pass the saved folder as `cwd`: the daemon does not restore it from the journal. The daemon's resolver also matches any id *containing* the text: resolve a name to an id first. A resume gets 25 s to attach (a long journal loads first), and one whose connection drops is sent again (idempotent on the daemon: a live target is attached), not looked up.
- `CreateOptions` gained `resume: Option<String>` / `.resume(..)` and `fallback_cwd: Option<Cwd>` / `.fallback_cwd(..)`: if the daemon refuses `cwd` as gone on its machine (`MISSING_CWD`, "cwd must be an absolute existing directory: …"), the create is sent once more in the fallback → new `Event::CwdFallback { attach_seq, missing, cwd }`. The folder is never checked locally (a forwarded socket's daemon is another machine).
- New `Client::saved_sessions(named_only, limit) -> Result<(), RequestError>` → `Event::SavedSessions` / `SavedSessionsFailed`. Sent only when `Welcome.features` lists `saved_sessions` (`FEATURE_SAVED_SESSIONS`): otherwise `Err(RequestError::Unsupported)` (`NotConnected` while the features are unknown) and nothing goes out. The connection thread checks again and answers `Error::Unsupported`.
- New `Expect { Unnamed, Named(String) }`, `RenameRefusal` (`Invalid`, `Taken`, `ChainConflict`, `Named`, `Locked`, `Busy`, `Unknown`, `Failed` from the wire; `NotLive` for `Refused{UnknownSession}`, `Unsupported` for an older daemon's `Error{unknown variant}`, `Offline` when nothing was sent) and `RenameFailure { session_id, reason, message, taken_by, current_name }`. `rename` → `Event::Renamed { session_id, name, in_memory_only }` or `Event::RenameFailed(RenameFailure)`; the name is checked first (`RequestError::InvalidName`).
- New `Client::features()` / `Client::supports(feature)`: the connected daemon's `Welcome.features` (`None` while down).
- New `Error::Unsupported { feature }`, `RequestError { Closed, NotConnected, Unsupported, InvalidName }`, `Command::{SavedSessions, Rename}`.
- Protocol mirrors: `ClientFrame::{SavedSessions { named_only, limit }, Rename { session_id, name, expect }}`, `DaemonFrame::{SavedSessionList { sessions, truncated, rejected }, Renamed { .. }, RenameFailed(RenameFailure)}`, `SavedSession` (`name`, `cwd`, `message_count` nullable; bad entries skipped and counted), `FEATURE_SAVED_SESSIONS`, `SAVED_SESSIONS_MAX`, `MISSING_CWD`, `SESSION_NAME_MAX`, `validate_session_name` / `InvalidSessionName` (agent-core's `validate_name`: `[a-z0-9-]{1,40}`).
- Control replies are matched to requests in order; this relies on the daemon awaiting `rename` and `saved_sessions` inline on the control connection (as `daemon/conn.rs` does today).

## API changes since 73a38f6

Follow-up to the PR #2 re-review (PR #3). Old → new, one line each.

- `Attached.pending_prompts` / `clients` / `roster` dropped undecodable entries silently → still skipped, but counted in the new `Attached.rejected: Rejected` (per list, `total()`), and the client emits `Event::Error { error: Error::Decode { kind: "attached", .. } }` right after that `Attached`.
- `Event::Session` could carry `SessionEvent::QueryResult` (other clients' replies, the daemon's reserved ids) → never does; `Client` drops every `QueryResult` that is not its own resync.
- `history::HistoryMirror::matches(&self, ..)` → `matches(&mut self, ..)`: a digest that matches by hash re-arms a distrusted hash.
- `HistoryMirror::update` under hash distrust returned `Append` for anything at least as long → always `Replace` until the hash is trusted again (as `absorb`).
- `Event::Detached` after `detach()` / a replacing attach had `session_id: None` → the session's id when known; a detach that cuts a create short says "it may exist now".
- `Event::Attached.mode` doc said `Takeover` after a reload → it is (and was) always the mode asked for.
- `Attached::may_drive` documented as an attach-time hint only (stale after `Event::InputOwner`, blind to owned sessions).
- `SessionEvent::Conversation` documented: in `Full` mode it can arrive ahead of the `Event::History` that resyncs to it.
- Reconnect backoff reset on every successful connect → only after the connection stayed up 10 s (control and session connections); a session connection that keeps dropping gives up after the reconnect budget.
- `attach()` / `create()` from several threads could enqueue `attach_seq`s out of order → enqueued in `attach_seq` order.

## API changes since a4d1937

For the synaps-avatar author: everything that changed shape, old → new, one line each.
Behaviour fixes with no API change are listed in PR #2.

### Construction and lifecycle
- `Client::spawn(path, identity) -> (Client, Receiver)` → `Client::spawn(ClientOptions::new(Endpoint::Socket(path) | Endpoint::Profile(p), identity)) -> io::Result<(Client, Receiver)>` (no panic on thread/runtime failure).
- `Client::connect(profile, identity) -> (..)` → `-> io::Result<(..)>`; `None` now means `$SYNAPS_PROFILE`, else the default daemon.
- Socket path resolved once → `Endpoint::Profile` re-reads `daemon[-profile].json` on every (re)connect.
- `Sessions` polled every 3 s always → no polling by default; opt in with `ClientOptions::poll_sessions(Duration)`.
- `Command::Shutdown` aborted the tasks → `Client::shutdown()` sends `Detach` + `Bye` on the session and `Bye` on the control connection; dropping every `Client` does the same.
- `Client::send(cmd)` (fire and forget) → `Client::send(cmd) -> Result<(), ClientClosed>`; every helper (`submit`, `abort`, ...) returns `Result` too.
- New helpers: `Client::steer`, `Client::answer`, `Client::resync`, `Client::shutdown`.
- Docs: each `Client` costs one OS thread + one current-thread tokio runtime + one control connection (one more while attached).

### Identity
- `Identity { pub name, pub client_version, pub instance, pub history, pub tail_items }` → private fields; `Identity::new(name) -> Result<Identity, InvalidName>` (Synaps' rule: 1–32 of `[A-Za-z0-9_-]`, starting alphanumeric), then `with_kind`, `with_version`, `with_history(mode, tail_items)`; getters `name()`, `kind()`, `history()`, `instance()`.
- `ClientMeta.kind: &'static str` (`"attach"`) → `ClientKind` enum (`Attach` by default).

### Attaching
- `client.attach(id)` (always `Mirror`) → `client.attach(id, AttachMode) -> Result<u64, ClientClosed>`; use `AttachMode::Observe` for a watcher (never owns input, never `Takeover` on reconnect).
- `client.create(model: Option<String>)` → `client.create(CreateOptions::new().mode(..).cwd(..).model(..).name(..)) -> Result<u64, ClientClosed>`.
- Created sessions ran in the process cwd → explicit `Cwd` policy, default `Cwd::Home` (`Cwd::Process`, `Cwd::Path(p)` also).
- `Command::Attach { session_id }` / `Command::Create { model }` → `Command::Attach { session_id, mode }` / `Command::Create(CreateOptions)`.
- Returned `u64` is the new `attach_seq`: every attachment event carries it; drop events whose `attach_seq` is not your latest.

### Events
- `Event` and `Status` are `#[non_exhaustive]`: add a `_` arm.
- `Event::Attached { attached, reconnect }` → `Event::Attached { attach_seq, attached, mode, reconnect }`; `Attached` now has `client`, `input_owner`, `client_input: ClientInput`, `roster: Vec<ClientInfo>`, `clients: Vec<(u64, ClientKind)>`, and `may_drive(mode)`.
- `Event::Session { session_id, seq, event }` → `Event::Session { attach_seq, session_id, seq, event, raw: Arc<Value> }` (`raw` = the wire event, every field).
- `MessageHistory` arrived as `Event::Session` → in `Full` mode it arrives as `Event::History { attach_seq, session_id, seq, update: HistoryUpdate }` (`Append { base_len, messages }` with only the new messages, or `Replace { messages }`).
- `Event::Detached { session_id, reason }` → `Event::Detached { attach_seq, session_id, reason }`; after it session commands fail with `Error::NotAttached`.
- `Event::Error(String)` → `Event::Error { attach_seq: Option<u64>, error: Error }` with `Error::{Refused { command, reason }, Daemon { session_id, message }, Transport(String), Decode(DecodeError), NotAttached { command }}`.
- New: `Event::InputOwner { attach_seq, session_id, client, owner, reason }`, `Event::Reattaching { attach_seq, session_id, reason }`, `Event::attach_seq()`.
- `Status::Reconnecting` was also sent by the session connection → `Status` is the control connection's alone; attachment state is `Attached` / `Reattaching` / `Detached`.
- New `Status::Incompatible(Incompatible)`: a protocol mismatch is no longer retried forever (`list_sessions()` tries again).

### Protocol mirrors (`protocol`)
- Every mirror enum (`SessionEvent`, `StreamEvent`, `LlmEvent`, `SessionStreamEvent`, `AgentEvent`, `DaemonFrame`, `ByeReason`, `ClientFrame`, `SessionCommand`) is `#[non_exhaustive]`.
- `Envelope::decode() -> SessionEvent` → `-> Result<SessionEvent, DecodeError>` (`Ok(Unknown)` = new variant, `Err` = known variant changed shape); new consuming `Envelope::into_event()`.
- `decode_event(&Value) -> SessionEvent` → `-> Result<SessionEvent, DecodeError>`.
- `SessionEvent::Conversation { digest: Value }` → `{ digest: ConversationDigest }` (`messages_len`, `messages_hash`, `cost`, `header`, ...).
- `SessionEvent::CompactionApplied` (unit) → `CompactionApplied { previous_session_id, session_id, msg_count }`.
- `SessionEvent::InputOwnerChanged { from, to }` → `{ from, to, reason }`; `ClientJoined` gained `mode`.
- New `SessionEvent` variants: `Refused { client, command, reason }`, `Cleared`, `Resumed`, `QueryResult { id, value }`, `CompactionCancelled`, `Lifecycle`.
- `DaemonFrame::SessionList(Vec<SessionMeta>)` → `DaemonFrame::SessionList { sessions, rejected }`; `Welcome` gained `rejected_sessions` (bad entries are skipped, not fatal).
- `SessionCommand` gained `Query { id, query: SessionQuery }` and `Detach { client }`; `CreateConfig` gained `name`.
- `Debug` of `Command`, `ClientFrame` and `SessionCommand` prints `<redacted>` for `Submit`/`Steer` text and `Answer` values.
- New: `history::{HistoryMirror, HistoryUpdate, messages_hash}` (Synaps' FNV-1a digest hash), `protocol::{refuse_kind, lenient, RESERVED_QUERY_ID_BASE, RELOADING_REFUSAL}`.

### Transcript
- `pub items / busy / title / pending_prompts / subagents / cost_usd` fields → read-only `items()`, `busy()`, `title()`, `pending_prompts()`, `subagents()`, `cost_usd()`; also `history_len()`, `session_id()`.
- `load_attached(&a)` → `load_attached(&a) -> Result<(), TranscriptError>` (refuses a `Digest` attach: `TranscriptError::DigestHistory`).
- New `apply_update(&HistoryUpdate) -> Result<(), TranscriptError>` for `Event::History` (`OutOfSync` → call `client.resync()`); `apply_history(&[Value])` stays for raw histories.
- Writing `transcript.busy = false` on detach → `transcript.set_detached()`.
- `ItemKind` and `ToolStatus` are `#[non_exhaustive]`; new `ToolStatus::Cancelled` (tools left open by an abort); `Item::is_note()` marks client-side notice/error rows, which now survive history refolds.
