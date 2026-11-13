# synaps-client changelog

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
