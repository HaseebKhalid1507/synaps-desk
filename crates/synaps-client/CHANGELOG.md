# synaps-client changelog

## Since a982132

Additive (PR `feat/client-set-system`), for synaps-avatar's persona. One struct-literal break: `CreateConfig` gained a field (a literal needs `..Default::default()`; nothing in this workspace builds one without it).

- New `CreateOptions::system` / `.system(text)` and `CreateConfig::system: Option<String>`: the new session's system prompt TEXT, sent as `config.system` (agent-engine `SessionConfig.system`, `EngineOpts.system`); absent when `None`. A create that attaches to a live session instead (a named retry, a live resume) does not apply it: use `Client::set`.
- New `Client::set(Setting) -> Result<u64, ClientClosed>` and `Command::Set { id, setting }`: the session's `Set{id, setting}` for `Setting::SystemPrompt { text }`, `Setting::Model { model }`, `Setting::ReasoningLevel { level }` (wire `{"setting":"system_prompt","text":…}`, `{"setting":"model","model":…}`, `{"setting":"reasoning_level","level":…}`). The returned id is random below `RESERVED_QUERY_ID_BASE`, then counting (the replies are broadcast to every attached client). The daemon keeps a Set in `settings_replay` (across a daemon reload) and in the journal (a resume restores it).
- New `Event::SettingApplied { attach_seq, session_id, id, setting, message, view }`: our `SettingChanged` with `ok: true`; `view` is the session's `RuntimeView` after it (as `Attached::view`). Another client's `SettingChanged` (and the daemon's `VIEW_CHANGED_SETTING_ID`) never becomes one; the raw `setting_changed` still comes as `Event::Session`, as before.
- New `Error::SettingRefused { id, setting, reason }` (`Error` is `#[non_exhaustive]`): a Set that was not applied: `ok: false` (the daemon's message), refused by the session (`Refused{command: "set"}`: only the input owner may set, so an `Observe` attach is refused; refusals carry no id, so it is the oldest Set still waiting), sent while attaching/reconnecting, or cut off by a dropped connection ("it may have applied": compare the next `Attached` view). A refused `set` no longer comes as `Error::Refused { command: "set" }` while a Set of ours is waiting.
- Tests: `tests/fixtures.rs` `settings_and_the_create_system_prompt_encode_as_the_daemon_expects`; `tests/fake_daemon.rs` `a_create_carries_the_system_prompt`, `set_is_answered_by_its_own_id`, `a_set_cut_off_by_a_drop_is_refused`.

For synaps-avatar (nyx): create her own session with `.system(persona)` and `.model(m)`, then `set(Setting::reasoning_level(..))` after the attach; on attaching to her existing session, set only what differs from `attached.view` (`system_prompt`, `model`, `reasoning_level`), from a `Mirror` attach.

## Since cedbdbd

Additive only (PR `polish/layout-backlog`), plus two behaviour fixes. Nothing existing changes shape: code that builds `Event::Attached { .. }` or matches `Command`, `ToolOutcome` or `ToolStatus` exhaustively (all `#[non_exhaustive]`) compiles as before.

- New `Attached::connection: u64` (not on the wire; `0` from serde, set by the client): this `Client`'s number for the connection the snapshot came on, new on every attach and every reconnect, never reused. `Attached` is `#[non_exhaustive]`, so no literal elsewhere builds it.
- New `Command::AnswerOn { connection, prompt_id, value }` and `Client::answer_on(connection, prompt_id, value)`: an `Answer` that is sent only on that connection. Once the session has reconnected (a reload, a restarted daemon that numbers its prompts from 1 again), it is dropped, never sent, and comes back as `Error::Refused { command: Some("answer"), .. }`; while reconnecting it is refused as every command is. `Client::answer` is unchanged (whatever connection is up when the thread gets to it). Its `Debug` redacts the value, as `Answer`'s does. Test: `tests/fake_daemon.rs` `answers_for_an_earlier_connection_are_never_sent`.
- Behaviour: `tool_outcome` calls an `activate_tools` error `Denied` only for the daemon's refusal, `tool activation requires host confirmation authority` (`HostActivationError::ConfirmationRequired`: the user denied the prompt, the policy is `deny`, or nothing could show one). Every other `tool activation denied: …` (a duplicate id, an already active tool, a digest or provenance error, a guessed id: failures after AUTHORIZE) is `Failed`.

- New `ToolOutcome::Unknown` and `ToolStatus::Unknown` (`is_terminal()` includes it). **Behaviour:** a result whose last line is the bash tool's output-limit notice is `Unknown`, not `Ok`: the daemon killed the command there and returns the result as a success anyway. Both forms: v0.11's `[output truncated at 262144]` and agent-engine `origin/dev`'s `output_limit_notice` (`[output truncated at N bytes: the command was stopped at this limit before it finished, so its exit status is unknown. …]`). Also when the notice ends the tail of a virtualised history result (`runtime/tool_spill.rs`: the tail, a blank line, then `[Output virtualised: …]`; the daemon's default above 12 KiB). Not detectable, so still `Ok`: the live `ToolResult` (cut to `max_tool_output`, 30 000 bytes by default, before the notice; the history that follows replaces it) and a history without virtualisation (also cut before the notice).
- The refusal of an `AnswerOn` for an earlier connection now says "the prompt was raised on an earlier connection; not sent" (true after another attach as well as after a reconnect).

For synaps-avatar (nyx): nothing to change. To make answers airtight, keep `attached.connection` from the last `Event::Attached` and answer with `client.answer_on(connection, id, value)`.

## Since 2ae846d

No API change. Two behaviour fixes in `transcript::tool_outcome` (PR `fix/review-pr5`):

- An `activate_tools` call whose confirmation was denied (`Tool execution failed: tool activation denied: …`, agent-engine `tools/discovery.rs` `activation_denied`) is `Denied`, not `Failed`. A guessed id (`… tool activation denied: unknown tool id …`) stays `Failed`.
- `Unknown tool` matches only the daemon's `Unknown tool: ` (with the colon), so a result that merely starts with the words is not a failure.

## API changes since deabaf1

Additive only (the hybrid UI, PR #5 `feat/hybrid-ui`), plus one behaviour fix (tool status from the result text).

- `ConversationSnapshot` gained `tokens: Value` (`serde(default)`): the attach snapshot's `ConversationTokens` (`{input, output, cache_read, cache_creation}`), which `ConversationDigest::tokens` already carried on every later `Conversation` event. `Null` from a daemon that omits it. A struct literal needs `..Default::default()` (nothing in this workspace builds one).
- `ConversationSnapshot` gained `keepwarm_tokens: Value` (`serde(default)`): the keep-warm ping tokens the daemon says are already included in `tokens`. `Null` from a daemon that predates them. (Not on `ConversationDigest`: that would grow every `Event::Session`; read it from the event's `raw` JSON, `digest.keepwarm_tokens`.)
- New `transcript::ToolOutcome { Ok, Failed, Denied, Cancelled }` (`#[non_exhaustive]`), `transcript::tool_outcome(text, is_error)` and `transcript::tool_result_outcome(&block)`: a finished tool call classified by its result text, as the Synaps TUI does. The engine leaves `is_error` unset on a failed tool (`Tool execution failed: …`), an extension block (a denied confirmation: `Tool call blocked by extension: …`), a gate denial (`Tool call denied: …`, `Unknown tool: …`) and a canceled call (`Canceled by user`, …), so `is_error` alone called all of them successes.
- New `ToolStatus::Denied` (`ToolStatus` is `#[non_exhaustive]`; `is_terminal()` includes it). **Behaviour:** `Transcript` now sets a tool row's status from `tool_outcome` (history and live `ToolResult`): those results were `Done`, now `Failed` / `Denied` / `Cancelled`.

## API changes since 355242c

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
