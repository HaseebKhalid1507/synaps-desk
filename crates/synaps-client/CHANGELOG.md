# synaps-client changelog

## API changes since 355242c

Named and saved sessions (PR #4). Additive except the one `Status` field; old → new, one line each.

- `Status::Connected { daemon_version, profile, pid }` → also `features: Vec<String>` (`Welcome.features`); match it with `..`.
- New `Client::resume(id_or_name, CreateOptions) -> Result<u64, ClientClosed>`: `Attach::Create` with `continue_session: Some(Some(id_or_name))` (every v3 daemon). Failures ("no session named 'x'", a journal live in another process) arrive as `Event::Detached { reason }`. A resume whose connection drops after the create is not re-sent: the retry attaches to the session it resumed (by name, id or journal id).
- `CreateOptions` gained `resume: Option<String>` / `.resume(..)`; `CreateConfig` gained `continue_session: Option<Option<String>>` (absent when `None`; `Some(None)` goes out as `null`, a bare `--continue`).
- New `Client::saved_sessions(named_only, limit) -> Result<(), RequestError>` → `Event::SavedSessions { sessions: Vec<SavedSession>, truncated }`. Sent only when `Welcome.features` lists `saved_sessions` (`FEATURE_SAVED_SESSIONS`): otherwise `Err(RequestError::Unsupported)` (`NotConnected` while the features are unknown) and nothing goes out. The connection thread checks again and answers `Event::Error { error: Error::Unsupported { feature } }`.
- New `Client::rename(session_id, Option<&str>, force) -> Result<(), RequestError>` on the control connection → `Event::Renamed { session_id, name, in_memory_only }` or `Event::RenameFailed(RenameFailure)`. The name is checked first (`RequestError::InvalidName`). Without `force` it sends `expect: null` (only an unnamed session is renamed; a named one is `RenameRefusal::Named` with `current_name`); `force` sends no `expect` (replace). Clearing (`None`) never has a precondition.
- New `RenameRefusal` (`Invalid`, `Taken`, `ChainConflict`, `Named`, `Locked`, `Busy`, `Unknown`, `Failed` from the wire; `NotLive` for `Refused{UnknownSession}`, `Unsupported` for an older daemon's `Error{unknown variant}`, `Offline` when nothing was sent) and `RenameFailure { session_id, reason, message, taken_by, current_name }`.
- New `Client::features()` / `Client::supports(feature)`: the connected daemon's `Welcome.features` (`None` while down).
- New `Error::Unsupported { feature }`, `RequestError { Closed, NotConnected, Unsupported, InvalidName }`, `Command::{SavedSessions, Rename}`.
- Protocol mirrors: `ClientFrame::{SavedSessions { named_only, limit }, Rename { session_id, name, expect }}`, `DaemonFrame::{SavedSessionList { sessions, truncated, rejected }, Renamed { .. }, RenameFailed(RenameFailure)}`, `SavedSession` (`name`, `cwd`, `message_count` nullable; bad entries skipped and counted), `FEATURE_SAVED_SESSIONS`, `SESSION_NAME_MAX`, `validate_session_name` / `InvalidSessionName` (agent-core's `validate_name`: `[a-z0-9-]{1,40}`).
- `SessionMeta` gained `awaiting_input`, `journal_id`, `locked_by` and `is(id)` (id or journal id).

## API changes since f2529da

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

## API changes since 10ebfe7

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
