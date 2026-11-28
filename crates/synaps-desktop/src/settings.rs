//! Settings without GPUI: every row of the TUI's `/settings` and the one
//! path that can really write it (`usage-design/spec.md` section 2).
//!
//! - APP: `settings.json` on this machine ([`crate::theme`]), always.
//! - SESSION: the running session. Shown from its `RuntimeView` (keep-warm
//!   from its `Status`); read-only here. Its model and thinking change from
//!   the composer's pickers ([`crate::choice`]), this session only.
//! - CONFIG: `<base>/config` on the daemon's machine, through synaps-dash's
//!   `POST /api/config` ([`crate::dash`]): only when the daemon is local and
//!   the dash says the file is writable. The desktop never writes or
//!   parses that file itself; the dash validates, locks and writes it.
//! - REFERENCE: the keys the dash does not expose, shown with the value a
//!   local daemon's config holds (`key = value`, last wins, only the TUI's
//!   own rows kept), never written.
//!
//! [`ALL`] is the TUI's `ALL_SETTINGS` (SynapsCLI 0.13.0, `013d1c4`,
//! `crates/agent-tui/src/tui/settings/defs.rs`): its 89 rows, labels and
//! help, in its order.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The TUI's categories (REFERENCE rows are sub-headed by them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Model,
    Agent,
    Context,
    Budgets,
    Tools,
    Startup,
    Daemon,
    Appearance,
}

impl Cat {
    /// The sub-header, tracked caps.
    pub fn title(self) -> &'static str {
        match self {
            Cat::Model => "M O D E L",
            Cat::Agent => "A G E N T",
            Cat::Context => "C O N T E X T   &   C A C H E",
            Cat::Budgets => "T U R N   B U D G E T S",
            Cat::Tools => "T O O L   L I M I T S",
            Cat::Startup => "S T A R T U P",
            Cat::Daemon => "D A E M O N",
            Cat::Appearance => "A P P E A R A N C E",
        }
    }

    /// The order REFERENCE lists them in.
    pub const ORDER: [Cat; 7] = [Cat::Model, Cat::Agent, Cat::Context, Cat::Budgets, Cat::Tools, Cat::Startup, Cat::Daemon];
}

/// The TUI's editor for a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctl {
    Choice(&'static [&'static str]),
    Number,
    Text,
    Model,
    /// Derived, never edited (`reasoning_type`).
    Display,
    Theme,
}

#[derive(Debug)]
pub struct Def {
    pub key: &'static str,
    pub label: &'static str,
    pub cat: Cat,
    pub ctl: Ctl,
    pub help: &'static str,
}

/// The levels the TUI's thinking cycler offers across models (the daemon
/// refuses what the model lacks).
pub const THINKING: &[&str] = &["off", "adaptive", "low", "medium", "high", "xhigh", "max"];

#[rustfmt::skip]
pub static ALL: [Def; 89] = [
    Def { key: "model", label: "Model", cat: Cat::Model, ctl: Ctl::Model, help: "Which Claude model to use." },
    Def { key: "thinking", label: "Thinking", cat: Cat::Model, ctl: Ctl::Choice(THINKING), help: "Thinking depth — controls effort on adaptive models, budget on legacy." },
    Def { key: "thinking_display", label: "Thinking display", cat: Cat::Model, ctl: Ctl::Choice(&["summarized", "omitted"]), help: "What Anthropic sends of the model's thinking. summarized (default): the thought's summary streams in, and the reply waits for it, then lands in a burst. omitted: no thinking text, just a timed 'thought' line, and the reply starts sooner and streams evenly (measured on Opus 5.5: ~2 s vs 3–5 s at high effort). Thinking effort is unchanged, and the reasoning still carries over between turns. Switching keeps the prompt cache. Applies to new sessions; workers always send summarized." },
    Def { key: "reasoning_type", label: "Reasoning", cat: Cat::Model, ctl: Ctl::Display, help: "How the active model expresses reasoning depth (derived from exact model capabilities; read-only)." },
    Def { key: "context_window", label: "Context window", cat: Cat::Model, ctl: Ctl::Choice(&["200k", "1m", "auto"]), help: "Override context window limit (auto = model default)." },
    Def { key: "compaction_model", label: "Compaction model", cat: Cat::Model, ctl: Ctl::Model, help: "Who writes /compact summaries: match (this chat's model; also empty, auto or default), provider-small (its provider's small model) or a model id. One that can't be used gives way to the chat's own model, with a notice." },
    Def { key: "api_retries", label: "API retries", cat: Cat::Agent, ctl: Ctl::Number, help: "Retries on transient API errors." },
    Def { key: "subagent_thinking", label: "Subagent thinking", cat: Cat::Agent, ctl: Ctl::Choice(&["low", "medium", "high", "xhigh", "max"]), help: "Default effort for new subagents; independent of foreground thinking. Resume keeps recorded effort unless overridden." },
    Def { key: "subagent_model", label: "Subagent model", cat: Cat::Agent, ctl: Ctl::Text, help: "Model for new subagents when a call omits model, e.g. claude-sonnet-4-6 or openai-codex/gpt-5.6-sol; inherit (default) = the foreground model. Naming a model trusts it for workers, as a favorite does; an unknown id refuses those dispatches instead of using the foreground model. Applies to workers started afterwards." },
    Def { key: "subagent_timeout", label: "Subagent timeout", cat: Cat::Agent, ctl: Ctl::Number, help: "Seconds before a dispatched subagent is canceled." },
    Def { key: "subagent_cold_resume_compact_tokens", label: "Cold resume compact", cat: Cat::Agent, ctl: Ctl::Number, help: "subagent_resume of a worker whose prompt cache expired replays a compact context instead of the full transcript once its history reaches this many tokens (default 100000; 0 = always replay in full). Applies from the next resume." },
    Def { key: "subagent_context_management", label: "Subagent rollover", cat: Cat::Agent, ctl: Ctl::Choice(&["auto", "off"]), help: "Automatic context rollover for subagent workers: auto moves a worker's older rounds out of its context once it reaches the rollover size (saved as plain text beside its resume archive, nothing summarized; the task and recent rounds stay); off lets a worker's context grow until the window is full (default auto). Applies to workers started afterwards." },
    Def { key: "subagent_rollover_tokens", label: "Subagent rollover at", cat: Cat::Agent, ctl: Ctl::Number, help: "Context size, in tokens, at which a worker rolls over to a ~100k successor (default 200000, at least 20000; a smaller model window's own threshold wins). Applies to workers started afterwards." },
    Def { key: "subagent_transform_hooks", label: "Subagent output hooks", cat: Cat::Agent, ctl: Ctl::Choice(&["on", "off"]), help: "Extensions that rewrite tool output (after_tool_call replace with tools.transform_output: compression, redaction) also see subagent workers' tool results, as they see the main session's; no other hook runs in a worker, so a worker never asks for confirmation (default on; off = worker tool results reach history untransformed). Applies to workers started afterwards." },
    Def { key: "max_tool_output", label: "Max tool output", cat: Cat::Tools, ctl: Ctl::Number, help: "Bytes to capture from a tool before truncating." },
    Def { key: "tool_output_spill_bytes", label: "Tool output spill", cat: Cat::Tools, ctl: Ctl::Number, help: "A tool result bigger than this many bytes is saved whole to a spill file; history keeps its start and end plus a pointer the model can read or grep (default 12288; 0 = off). Applies to new sessions and workers." },
    Def { key: "tool_output_spill_head_bytes", label: "Spill preview head", cat: Cat::Tools, ctl: Ctl::Number, help: "Bytes from the start of a spilled tool result kept in history (default 4096)." },
    Def { key: "tool_output_spill_tail_bytes", label: "Spill preview tail", cat: Cat::Tools, ctl: Ctl::Number, help: "Bytes from the end of a spilled tool result kept in history, where errors usually are (default 3072)." },
    Def { key: "read_dedupe", label: "Read dedupe", cat: Cat::Tools, ctl: Ctl::Choice(&["on", "off"]), help: "A re-read of a file whose result would be identical to an earlier read still in context returns a short stub pointing at that earlier result, and write/edit results say there is no need to re-read to verify (default on; off = full results every time). Applies to new sessions and workers." },
    Def { key: "bash_timeout", label: "Bash timeout", cat: Cat::Tools, ctl: Ctl::Number, help: "Default seconds allowed for a bash command." },
    Def { key: "bash_max_timeout", label: "Bash max timeout", cat: Cat::Tools, ctl: Ctl::Number, help: "Legacy setting retained for config compatibility; requested bash timeouts are no longer clamped." },
    Def { key: "theme", label: "Theme", cat: Cat::Appearance, ctl: Ctl::Theme, help: "Color theme (restart required)." },
    Def { key: "tui_background_opaque", label: "Background", cat: Cat::Appearance, ctl: Ctl::Choice(&["opaque", "invisible"]), help: "Opaque paints Synaps' theme background; invisible uses your terminal background." },
    Def { key: "tui_streaming_glow", label: "Streaming glow", cat: Cat::Appearance, ctl: Ctl::Choice(&["on", "off"]), help: "A soft glow in the context bar's colour sweeps across the prompt while a reply streams." },
    Def { key: "tui_smooth_streaming", label: "Smooth streaming", cat: Cat::Appearance, ctl: Ctl::Choice(&["on", "off"]), help: "When a reply's text arrives in a burst (the provider flushing a backlog at once), reveal it over a few frames instead of in one, catching up within half a second (default on). Off = text paints as it arrives." },
    Def { key: "theme_transition", label: "Theme transition", cat: Cat::Appearance, ctl: Ctl::Choice(&["on", "off"]), help: "Animated cross-fade on theme changes (on = 350ms). Off = instant snap. Integer ms (0-2000) accepted in the config file." },
    Def { key: "startup.quick_start", label: "Quick start", cat: Cat::Startup, ctl: Ctl::Choice(&["on", "off"]), help: "Start without waiting for extensions to load (faster startup). The first message still waits for extensions to start before it is sent (up to the extensions-ready wait, 30 s by default, plus the extensions' session-start hooks). Takes effect next launch." },
    Def { key: "startup.extensions_ready_timeout_secs", label: "Extensions-ready wait", cat: Cat::Startup, ctl: Ctl::Number, help: "Max seconds to wait for extension discovery: at startup when Quick Start is off, before the first message is sent when it is on. Takes effect next launch." },
    Def { key: "daemon.idle_exit_secs", label: "Daemon idle timeout", cat: Cat::Daemon, ctl: Ctl::Number, help: "Seconds the daemon stays alive after the last client disconnects (higher = stays warm, fewer cold starts). 0 = never idle-exit. Env SYNAPS_DAEMON_IDLE_EXIT_SECS overrides." },
    Def { key: "daemon.prompt_abandon_secs", label: "Prompt-abandon timeout", cat: Cat::Daemon, ctl: Ctl::Number, help: "Seconds a pending confirmation on a detached session survives before it is auto-denied. 0 = disabled (survives forever). Env SYNAPS_DAEMON_PROMPT_ABANDON_SECS overrides." },
    Def { key: "daemon.parked_evict_secs", label: "Parked eviction", cat: Cat::Daemon, ctl: Ctl::Number, help: "Seconds a parked session row lingers in the daemon before eviction (its state stays on disk; --continue rebuilds it). 0 = keep forever. Env SYNAPS_DAEMON_PARKED_EVICT_SECS overrides." },
    Def { key: "client_input", label: "Client input", cat: Cat::Daemon, ctl: Ctl::Choice(&["shared", "exclusive"]), help: "Who may type in a daemon session with several clients attached (tabs, a dashboard, a bridge): shared (default) lets every client except --observe ones send input; exclusive lets only the input owner, and the others /takeover. The daemon reads it when it starts: `synaps daemon reload` applies it." },
    Def { key: "cache_ttl", label: "Cache TTL", cat: Cat::Context, ctl: Ctl::Choice(&["auto", "5m", "1h", "hybrid"]), help: "Prompt-cache lifetime for Anthropic requests: auto (default: 1h on a Claude subscription login, 5m on an API key, where 1h writes cost 2x input), 5m, 1h on every marker, or hybrid (1h on the stable prefix, 5m on the message tail). Subagent workers always use 5m. Takes effect next launch." },
    Def { key: "cache_diagnostics", label: "Cache diagnostics", cat: Cat::Context, ctl: Ctl::Choice(&["on", "off"]), help: "Opt into the cache-diagnosis beta (reports why a prompt-cache read missed). Takes effect next launch." },
    Def { key: "cache_keepwarm", label: "Cache keep-warm", cat: Cat::Context, ctl: Ctl::Choice(&["off", "auto", "1h", "2h", "4h", "8h", "12h", "24h", "48h"]), help: "Keep the prompt cache warm while idle: off, auto (stops once pings would cost more than the rewrite they prevent), or stop after <N>h (1–48). ←/→ cycles presets; Enter types any <N>h. New sessions after next launch; /keepwarm changes the current session." },
    Def { key: "cache_keepwarm_auto_max_hours", label: "Keep-warm auto max (h)", cat: Cat::Context, ctl: Ctl::Number, help: "Longest an auto keep-warm idle period may run, in hours (1–48, default 24). Takes effect next launch." },
    Def { key: "cache_keepwarm_headless", label: "Keep-warm headless", cat: Cat::Context, ctl: Ctl::Choice(&["off", "on"]), help: "Let daemon sessions driven only by chat, rpc or server clients (bridges, scripts) keep warm too; off (default) keeps it to sessions a TUI or `synaps attach` typed in. `synaps rpc` without the daemon never keeps warm. Takes effect next launch." },
    Def { key: "cache_keepwarm_tools", label: "Keep-warm tool rounds", cat: Cat::Context, ctl: Ctl::Choice(&["on", "off"]), help: "Keep the prompt cache warm through a long round: a response that streams past the cache TTL (minutes of thinking, one long write) or tools that run past it (builds, test sweeps, polls). Once the round has run for the TTL minus a margin (4.5 min on 5m, 55 min on 1h), resend its request asking for a minimal reply so the next request reads the cache instead of rewriting it. Independent of idle keep-warm (default on). Applies to new sessions and workers." },
    Def { key: "context_management.mode", label: "Context management", cat: Cat::Context, ctl: Ctl::Choice(&["off", "auto"]), help: "Automatic context management: auto plans a wrap-up and hands over to a fresh context before the window fills; off disables it (default). Takes effect next launch." },
    Def { key: "context_management.pressure_tokens", label: "Pressure threshold", cat: Cat::Context, ctl: Ctl::Text, help: "Token count where context pressure starts (auto = window-aware default: 140k on 200k, 250k on 1m). Must stay below the rollover threshold. Takes effect next launch." },
    Def { key: "context_management.rollover_tokens", label: "Rollover threshold", cat: Cat::Context, ctl: Ctl::Text, help: "Token count where a context rollover becomes due (auto = window-aware default: 180k on 200k, 400k on 1m). Takes effect next launch." },
    Def { key: "context_management.target_tokens", label: "Rollover target", cat: Cat::Context, ctl: Ctl::Text, help: "Context size a rollover aims for, as reported by the provider when known (auto = min(100k, 2/3 of pressure)). Must stay below the pressure threshold. Takes effect next launch." },
    Def { key: "context_management.reserve_tokens", label: "Reserve tokens", cat: Cat::Context, ctl: Ctl::Number, help: "Minimum free tokens kept before any model round (1–1000000, default 16000). Takes effect next launch." },
    Def { key: "context_management.finish_rounds", label: "Finish rounds", cat: Cat::Context, ctl: Ctl::Number, help: "Extra plan/wrap-up rounds allowed after a rollover becomes due (0–8, default 2; 0 = none). Takes effect next launch." },
    Def { key: "compaction_mode", label: "Compaction mode", cat: Cat::Context, ctl: Ctl::Choice(&["remote", "local"]), help: "Where /compact summarises: remote (the configured provider, default) or local (no network request is built). Takes effect next launch." },
    Def { key: "compaction_exclude", label: "Compaction exclude", cat: Cat::Context, ctl: Ctl::Text, help: "Content kept out of remote compaction, comma-separated: user_text, assistant_text, thinking, tool_calls, tool_results, file_paths, event_data. Empty = none. Takes effect next launch." },
    Def { key: "refusal_retries", label: "Refusal retries", cat: Cat::Context, ctl: Ctl::Number, help: "Retries when the model stops with a refusal (default 2). Takes effect next launch." },
    Def { key: "telemetry", label: "Telemetry", cat: Cat::Context, ctl: Ctl::Choice(&["off", "basic", "full"]), help: "Per-request API log in ~/.cache/synaps/api-log.jsonl (stays on this machine): off (default), basic (timing, usage, cost) or full (adds rate-limit headers and cache diagnostics). Takes effect next launch." },
    Def { key: "telemetry_log_generations", label: "Telemetry log files", cat: Cat::Context, ctl: Ctl::Number, help: "Rotated files kept for api-log.jsonl and request-trace.jsonl, each up to 50 MB: <log>.1 (newest) to <log>.N (1–100, default 1). Takes effect next launch." },
    Def { key: "request_compression", label: "Request compression", cat: Cat::Context, ctl: Ctl::Choice(&["auto", "on", "off"]), help: "gzip Anthropic request bodies of 16 KiB or more (every request re-uploads the whole conversation; text shrinks about 2x): auto (default) only when sent straight to the Anthropic API: no HTTPS proxy, custom CA bundle or custom base URL; on also through those, for a proxy or gateway known to pass gzip bodies; off never. A refused body is resent uncompressed. Env SYNAPS_GZIP_REQUEST_BODIES=0 overrides. Takes effect next launch." },
    Def { key: "request_compression_level", label: "Compression level", cat: Cat::Context, ctl: Ctl::Choice(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]), help: "gzip level for request bodies, 1-9 (default 1, about 250 MB/s). Higher shrinks more and costs more CPU: 1 suits fast uplinks; on an uplink slower than roughly 10-50 Mbps (it depends on the conversation) 3-6 saves more upload time than it costs. Above 6 gains almost nothing. Takes effect next launch." },
    Def { key: "subagent.max_concurrent", label: "Subagents at once", cat: Cat::Agent, ctl: Ctl::Number, help: "Workers running at the same time across the whole delegation tree (at least 1, default 16; never above the total). Takes effect next launch." },
    Def { key: "subagent.max_total", label: "Subagents in total", cat: Cat::Agent, ctl: Ctl::Number, help: "Workers dispatched and not yet reconciled across the delegation tree (at least 1, default 64). Takes effect next launch." },
    Def { key: "subagent.max_depth", label: "Subagent depth", cat: Cat::Agent, ctl: Ctl::Number, help: "How deep workers may nest: 1 = only the foreground delegates (1–65535, default 4). Takes effect next launch." },
    Def { key: "session_persistence", label: "Session persistence", cat: Cat::Agent, ctl: Ctl::Choice(&["json", "journal"]), help: "How sessions are saved: json (full rewrite, default) or journal (append-only deltas with periodic snapshots). Applies from the next session save." },
    Def { key: "agent_name", label: "Agent name", cat: Cat::Agent, ctl: Ctl::Text, help: "Name the agent goes by. Takes effect next launch." },
    Def { key: "identity", label: "Identity", cat: Cat::Agent, ctl: Ctl::Text, help: "Identity preamble at the top of the system prompt (default: the SynapsCLI identity). Remove the line from the config to restore the default. Takes effect next launch." },
    Def { key: "events.auto_turn", label: "Auto turns", cat: Cat::Agent, ctl: Ctl::Choice(&["on", "off"]), help: "Start a model turn automatically when runtime events arrive while idle (server, RPC and daemon sessions). Takes effect next launch." },
    Def { key: "events.auto_turn_cap", label: "Auto-turn cap", cat: Cat::Agent, ctl: Ctl::Number, help: "Consecutive automatic turns before the session waits for real input (default 5; 0 = unlimited). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_cost_usd", label: "Foreground cost (USD)", cat: Cat::Budgets, ctl: Ctl::Text, help: "US dollars one turn may spend before it stops (decimal, e.g. 2.50) — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_elapsed_secs", label: "Foreground time (s)", cat: Cat::Budgets, ctl: Ctl::Number, help: "Wall-clock seconds one turn may run; 0 = no limit (the default) — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_tool_calls", label: "Foreground tool calls", cat: Cat::Budgets, ctl: Ctl::Text, help: "Tool calls allowed in one turn; a positive number, or unlimited (default: no limit) — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_provider_rounds", label: "Foreground rounds", cat: Cat::Budgets, ctl: Ctl::Text, help: "Model round-trips allowed in one turn before the allowance runs out; a positive number, or unlimited (default: no limit) — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_round_renewals", label: "Foreground renewals", cat: Cat::Budgets, ctl: Ctl::Number, help: "Automatic renewals of the round allowance after it runs out mid-turn (only with a round limit set); 0 = hard stop at the first exhaustion — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_context_tokens", label: "Foreground ctx tokens", cat: Cat::Budgets, ctl: Ctl::Number, help: "Context tokens one turn may use — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.foreground.max_accumulated_tool_result_bytes", label: "Foreground tool bytes", cat: Cat::Budgets, ctl: Ctl::Text, help: "Total bytes of tool results one turn may accumulate; a positive number, or unlimited (default: no limit) — turns you type or queue, plugin commands and session-driver turns, in the TUI, daemon sessions, synaps chat and rpc (event-started turns also apply the event budget). Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_cost_usd", label: "Event cost (USD)", cat: Cat::Budgets, ctl: Ctl::Text, help: "US dollars one turn may spend before it stops (decimal, e.g. 2.50) — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_elapsed_secs", label: "Event time (s)", cat: Cat::Budgets, ctl: Ctl::Number, help: "Wall-clock seconds one turn may run; 0 = no limit (the default) — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_tool_calls", label: "Event tool calls", cat: Cat::Budgets, ctl: Ctl::Text, help: "Tool calls allowed in one turn; a positive number, or unlimited (default: 96) — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_provider_rounds", label: "Event rounds", cat: Cat::Budgets, ctl: Ctl::Text, help: "Model round-trips allowed in one turn before the allowance runs out; a positive number, or unlimited (default: 24) — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_round_renewals", label: "Event renewals", cat: Cat::Budgets, ctl: Ctl::Number, help: "Automatic renewals of the round allowance after it runs out mid-turn (only with a round limit set); 0 = hard stop at the first exhaustion — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_context_tokens", label: "Event ctx tokens", cat: Cat::Budgets, ctl: Ctl::Number, help: "Context tokens one turn may use — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.event.max_accumulated_tool_result_bytes", label: "Event tool bytes", cat: Cat::Budgets, ctl: Ctl::Text, help: "Total bytes of tool results one turn may accumulate; a positive number, or unlimited (default: 8 MiB) — turns a session starts by itself when events arrive (inbox messages, subagent completions); combined with the session budget, the stricter limit wins. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_cost_usd", label: "Worker cost (USD)", cat: Cat::Budgets, ctl: Ctl::Text, help: "US dollars one turn may spend before it stops (decimal, e.g. 2.50) — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_elapsed_secs", label: "Worker time (s)", cat: Cat::Budgets, ctl: Ctl::Number, help: "Wall-clock seconds one turn may run; 0 = no limit (the default) — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_tool_calls", label: "Worker tool calls", cat: Cat::Budgets, ctl: Ctl::Text, help: "Tool calls allowed in one turn; a positive number, or unlimited (default: no limit) — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_provider_rounds", label: "Worker rounds", cat: Cat::Budgets, ctl: Ctl::Text, help: "Model round-trips allowed in one turn before the allowance runs out; a positive number, or unlimited (default: 64 per allowance) — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_round_renewals", label: "Worker renewals", cat: Cat::Budgets, ctl: Ctl::Number, help: "Automatic renewals of the round allowance after it runs out mid-turn (only with a round limit set); 0 = hard stop at the first exhaustion — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_context_tokens", label: "Worker ctx tokens", cat: Cat::Budgets, ctl: Ctl::Number, help: "Context tokens one turn may use — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "turn_budget.worker.max_accumulated_tool_result_bytes", label: "Worker tool bytes", cat: Cat::Budgets, ctl: Ctl::Text, help: "Total bytes of tool results one turn may accumulate; a positive number, or unlimited (default: no limit) — subagent workers; each start or resume gets a fresh allowance. Unset = the role's built-in default (remove the line from the config to restore it). Takes effect next launch." },
    Def { key: "progressive_tool_disclosure", label: "Progressive tools", cat: Cat::Tools, ctl: Ctl::Choice(&["on", "off"]), help: "Start each session with a small core tool set and activate the rest on demand (off = send every tool schema, default). Takes effect next launch." },
    Def { key: "tools.deferred_loading", label: "Deferred tool loading", cat: Cat::Tools, ctl: Ctl::Choice(&["on", "off"]), help: "With progressive tools on the native Anthropic transport: send activatable tools up front as deferred so an activation never changes the cached prompt (default on). Takes effect next launch." },
    Def { key: "tools.activation_confirm", label: "Tool activation", cat: Cat::Tools, ctl: Ctl::Choice(&["auto", "prompt", "deny"]), help: "When the model asks to activate tools: auto allows (default), prompt asks you, deny refuses. Takes effect next launch." },
    Def { key: "tools.always_loaded", label: "Always-loaded tools", cat: Cat::Tools, ctl: Ctl::Text, help: "With progressive tools on: tools kept loaded from the first request, comma-separated ids as activate_tools takes them (e.g. ext.axel:search, builtin:subagent). Ids not installed are skipped. Empty = none. Takes effect next launch." },
    Def { key: "process_scopes", label: "Process scopes", cat: Cat::Tools, ctl: Ctl::Choice(&["auto", "off"]), help: "Run the daemon, bash commands and shells in their own systemd scopes so the OOM killer stops one runaway command, not the terminal (Linux; default auto). Applies to commands started afterwards; the daemon's own scope changes next launch. Env SYNAPS_PROCESS_SCOPES overrides." },
    Def { key: "disabled_tools", label: "Disabled tools", cat: Cat::Tools, ctl: Ctl::Text, help: "Built-in tools never offered to the model, comma-separated runtime names (e.g. bash, ls). Empty = none. Takes effect next launch." },
    Def { key: "disabled_skills", label: "Disabled skills", cat: Cat::Tools, ctl: Ctl::Text, help: "Skills never loaded, comma-separated names. Empty = none. Takes effect next launch." },
    Def { key: "max_fps", label: "Max FPS", cat: Cat::Appearance, ctl: Ctl::Number, help: "Redraw cap while streaming, in frames per second (1–1000, default 60). Input always redraws immediately. Takes effect next launch." },
    Def { key: "scroll_lines", label: "Scroll lines", cat: Cat::Appearance, ctl: Ctl::Number, help: "Lines scrolled per mouse-wheel event (1–20, default 3). Takes effect next launch." },
];

/// Rows that configure the terminal UI only (the desktop has no glow, no
/// transition and its own scrolling): not shown.
pub const TERMINAL_ONLY: [&str; 6] =
    ["tui_background_opaque", "tui_streaming_glow", "tui_smooth_streaming", "theme_transition", "max_fps", "scroll_lines"];
/// Derived from the model capability catalog, which the desktop does not have.
pub const DERIVED: &str = "reasoning_type";

/// The live session's settings (its `RuntimeView`'s), in the TUI's order:
/// the config key that is their default for new sessions, and its label.
pub const SESSION_KEYS: [&str; 9] = [
    "model",
    "thinking",
    "context_window",
    "compaction_model",
    "api_retries",
    "subagent_timeout",
    "max_tool_output",
    "bash_timeout",
    "bash_max_timeout",
];

/// DAEMON CONFIG: what synaps-dash exposes of the TUI's rows (its
/// `CONFIG_KEYS` with a validator), sub-headed by the TUI's categories.
pub const CONFIG_GROUPS: [(Cat, &[&str]); 6] = [
    (Cat::Model, &["model", "thinking", "context_window", "compaction_model"]),
    (Cat::Agent, &["api_retries", "subagent_timeout", "events.auto_turn", "events.auto_turn_cap"]),
    (Cat::Context, &["cache_ttl", "context_management.mode", "compaction_mode", "refusal_retries"]),
    (Cat::Tools, &["max_tool_output", "bash_timeout", "bash_max_timeout", "progressive_tool_disclosure", "tools.activation_confirm"]),
    (Cat::Startup, &["startup.quick_start", "startup.extensions_ready_timeout_secs"]),
    (Cat::Daemon, &["daemon.idle_exit_secs", "daemon.prompt_abandon_secs", "daemon.parked_evict_secs"]),
];

pub fn def(key: &str) -> Option<&'static Def> {
    ALL.iter().find(|d| d.key == key)
}

pub fn is_config(key: &str) -> bool {
    CONFIG_GROUPS.iter().any(|(_, keys)| keys.contains(&key))
}

/// The REFERENCE rows of `cat`: shown, not the theme, not in CONFIG.
pub fn reference(cat: Cat) -> impl Iterator<Item = &'static Def> {
    ALL.iter().filter(move |d| d.cat == cat && d.key != "theme" && d.key != DERIVED && !TERMINAL_ONLY.contains(&d.key) && !is_config(d.key))
}

/// What a CONFIG control offers, as the TUI labels it; `None`: a number or
/// a model. The dash's validators are narrower than the TUI's: no `auto`
/// for `cache_ttl` and `context_window` (`auto` is sent as unset), and
/// `true` / `false` for two on/off keys ([`to_dash`]).
pub fn config_choices(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "thinking" => THINKING,
        "context_window" => &["200k", "1m", "auto"],
        "cache_ttl" => &["auto", "5m", "1h", "hybrid"],
        "context_management.mode" => &["off", "auto"],
        "compaction_mode" => &["remote", "local"],
        "progressive_tool_disclosure" | "events.auto_turn" | "startup.quick_start" => &["on", "off"],
        "tools.activation_confirm" => &["auto", "prompt", "deny"],
        _ => return None,
    })
}

/// Keys whose `auto` is the key unset (the dash has no `auto`).
fn auto_is_unset(key: &str) -> bool {
    matches!(key, "cache_ttl" | "context_window")
}

/// Keys the dash takes as `true` / `false`, shown on / off.
fn boolean(key: &str) -> bool {
    matches!(key, "progressive_tool_disclosure" | "events.auto_turn")
}

/// The value to send for what the user picked or typed (`None`: unset).
pub fn to_dash(key: &str, label: &str) -> Option<String> {
    let v = label.trim();
    match v {
        "" => None,
        "auto" if auto_is_unset(key) => None,
        "on" if boolean(key) => Some("true".into()),
        "off" if boolean(key) => Some("false".into()),
        v => Some(v.to_string()),
    }
}

/// A config value as shown: the TUI's label; unset is `default` (`auto`
/// where unset means it, `inherit` for `subagent_model`: the foreground
/// model, as the TUI shows it).
pub fn shown(key: &str, raw: Option<&str>) -> String {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None if auto_is_unset(key) => "auto".into(),
        None if key == "subagent_model" => "inherit".into(),
        None => "default".into(),
        Some("true") if boolean(key) => "on".into(),
        Some("false") if boolean(key) => "off".into(),
        Some(v) => v.to_string(),
    }
}

/// A number's unit, as the TUI's help states it.
pub fn unit(key: &str) -> &'static str {
    if key == "max_tool_output" || key.ends_with("_bytes") {
        "bytes"
    } else if key.ends_with("_secs") || key.ends_with("timeout") {
        "s"
    } else if key.ends_with("_tokens") {
        "tokens"
    } else {
        ""
    }
}

/// The tag under a row whose change is not immediate (the dash's `applies`).
pub fn applies_tag(applies: &str) -> Option<&'static str> {
    match applies {
        "reload" => Some("AFTER RELOAD"),
        "start" => Some("NEXT DAEMON START"),
        "launch" => Some("NEXT LAUNCH"),
        _ => None,
    }
}

/// The help's first sentence (the rest shows when the row is clicked).
pub fn first_sentence(help: &str) -> &str {
    let mut from = 0;
    while let Some(i) = help[from..].find(". ") {
        let end = from + i + 1;
        // Not after an abbreviation ("e.g. 2.50", "i.e.").
        if !help[..end].ends_with("e.g.") && !help[..end].ends_with("i.e.") {
            return &help[..end];
        }
        from = end + 1;
    }
    help
}

/// The bytes of the help's sentence that holds byte `byte` (a find's hit),
/// its sentences as [`first_sentence`] ends them.
pub fn sentence_at(help: &str, byte: usize) -> std::ops::Range<usize> {
    let (mut start, mut from) = (0, 0);
    while let Some(i) = help[from..].find(". ") {
        let end = from + i + 1;
        from = end + 1;
        if help[..end].ends_with("e.g.") || help[..end].ends_with("i.e.") {
            continue;
        }
        if byte < end {
            return start..end;
        }
        start = from;
    }
    start..help.len()
}

/// `key = value` lines, the last of a key winning, as Synaps' parser and
/// the dash read them; comments and other lines skipped. Only the TUI's
/// own rows are kept (an allowlist: no `bridge.*`, `shell.*`, extension
/// keys or secrets are held), for the REFERENCE values of a local daemon
/// (never written back).
pub fn read_config(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())).filter(|(k, _)| def(k).is_some() && !secret(k)) {
            m.insert(k.to_string(), v.to_string());
        }
    }
    m
}

/// A local daemon's config for REFERENCE: no file is all defaults; a file
/// that cannot be read is `Err` (its rows show `—`).
pub fn read_reference(base: &Path, profile: Option<&str>) -> Reference {
    let path = crate::theme::read_path(base, profile, "config");
    let values = match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Arc::new(read_config(&text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Arc::default()),
        Err(e) => Err(e.to_string()),
    };
    Reference { path: crate::named::home_short(&path.display().to_string()), values }
}

/// A local daemon's config as read: the file (the profile's, else the
/// base one, as Synaps finds it; `~`-shortened) and its values.
#[derive(Clone, Debug)]
pub struct Reference {
    pub path: String,
    pub values: Result<Arc<HashMap<String, String>>, String>,
}

/// A CONFIG number's range, as synaps-dash's `CONFIG_KEYS` takes it
/// (`intIn(lo, hi)`: 1 to 9 digits, inclusive).
pub fn range(key: &str) -> Option<(u64, u64)> {
    Some(match key {
        "api_retries" => (0, 20),
        "refusal_retries" => (0, 10),
        "subagent_timeout" => (10, 86_400),
        "max_tool_output" => (1024, 4 << 20),
        "bash_timeout" => (1, 3600),
        "bash_max_timeout" => (1, 86_400),
        "events.auto_turn_cap" => (0, 1000),
        "daemon.idle_exit_secs" | "daemon.prompt_abandon_secs" | "daemon.parked_evict_secs" => (0, 604_800),
        "startup.extensions_ready_timeout_secs" => (1, 600),
        _ => return None,
    })
}

/// Secrets never shown, even in REFERENCE (none of its rows is one; the
/// file holds them).
pub fn secret(key: &str) -> bool {
    key.starts_with("auth.")
        || key.starts_with("provider.")
        || key.starts_with("server.")
        || key.contains("token") && !key.ends_with("_tokens")
}

/// A typed value checked before it is sent, by the dash's own rules (its
/// own refusal, worded its way, still comes back for anything else):
/// `None` is fine.
pub fn check(key: &str, typed: &str) -> Option<String> {
    let v = typed.trim();
    if v.is_empty() {
        return None;
    }
    let whole = (1..=9).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit());
    match def(key).map(|d| d.ctl) {
        Some(Ctl::Model) if !model_ok(v) => Some(format!("{key}: expected provider/model, got {v:?}")),
        Some(Ctl::Number) if !whole => Some(format!("{key}: expected a whole number, got {v:?}")),
        Some(Ctl::Number) => match (range(key), v.parse::<u64>()) {
            (Some((lo, hi)), Ok(n)) if n < lo || n > hi => Some(format!("{key}: expected {lo} to {hi}, got {n}")),
            _ => None,
        },
        Some(Ctl::Choice(opts)) if !opts.contains(&v) && config_choices(key).is_none_or(|c| !c.contains(&v)) => {
            Some(format!("{key}: expected one of {}", opts.join(", ")))
        }
        _ => None,
    }
}

/// What the dash-backed editor of `key` takes where the row's help (the
/// CLI's, kept as it is) names more, shown beside the editor: synaps-dash's
/// validator and [`check`] take provider/model or empty (unset) for
/// `compaction_model`, not its policy words (Shady #107 P3-1).
pub fn editor_note(key: &str) -> Option<&'static str> {
    match key {
        "compaction_model" => Some(
            "This editor takes provider/model, or empty to unset. match, provider-small, auto and default can be set only in the config file or the TUI for now.",
        ),
        _ => None,
    }
}

/// A model setting's value checked as the dash does (`provider/model`).
pub fn model_ok(v: &str) -> bool {
    let ok = |s: &str, extra: &[char]| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || extra.contains(&c))
    };
    v.split_once('/').is_some_and(|(p, m)| ok(p, &[]) && ok(m, &[':']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tui_row_has_one_place() {
        assert_eq!(ALL.len(), 89, "SynapsCLI 0.13.0's ALL_SETTINGS (013d1c4): 89 rows");
        let config: Vec<&str> = CONFIG_GROUPS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
        assert_eq!(config.len(), 22, "22 rows through the dash");
        for k in &config {
            assert!(def(k).is_some(), "{k} is a TUI row");
        }
        for k in SESSION_KEYS {
            assert!(config.contains(&k), "{k}: a session row's default is a CONFIG row");
        }
        let reference: usize = Cat::ORDER.iter().map(|&c| reference(c).count()).sum();
        assert_eq!(reference, 59, "59 reference rows");
        // 89 = 22 CONFIG + 59 REFERENCE + theme (APP) + 6 terminal-only + 1 derived.
        assert_eq!(22 + reference + 1 + TERMINAL_ONLY.len() + 1, ALL.len());
        let mut keys: Vec<&str> = ALL.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 89, "no key twice");
        assert!(ALL.iter().all(|d| !secret(d.key)), "no row is a secret");
        // 0.13.0's new row: Agent, read here (the dash doesn't write it),
        // where the TUI lists it: after subagent_thinking.
        let agent: Vec<&str> = super::reference(Cat::Agent).map(|d| d.key).collect();
        assert!(agent.windows(2).any(|w| w == ["subagent_thinking", "subagent_model"]), "{agent:?}");
        assert!(!is_config("subagent_model"));
        // And its reworded help.
        assert!(def("compaction_model").is_some_and(|d| d.help.starts_with("Who writes /compact summaries: match (this chat's model")));
        // THIS SESSION: RuntimeView's settings, in the TUI's order, each
        // with its default for new sessions in CONFIG (checked above).
        assert_eq!(
            SESSION_KEYS.as_slice(),
            &[
                "model",
                "thinking",
                "context_window",
                "compaction_model",
                "api_retries",
                "subagent_timeout",
                "max_tool_output",
                "bash_timeout",
                "bash_max_timeout"
            ]
        );
    }

    /// The TUI's `subagent_model_row_shows_inherit_when_unset`: unset, blank
    /// or `inherit` is `inherit` (the foreground model); else as written.
    #[test]
    fn subagent_model_shows_inherit_when_unset() {
        for (raw, says) in
            [(None, "inherit"), (Some("inherit"), "inherit"), (Some("   "), "inherit"), (Some("claude-sonnet-4-6"), "claude-sonnet-4-6")]
        {
            assert_eq!(shown("subagent_model", raw), says, "{raw:?}");
        }
        assert_eq!(shown("subagent_thinking", None), "default", "only subagent_model inherits");
    }

    #[test]
    fn dash_values_and_tui_labels_round_trip() {
        assert_eq!(to_dash("cache_ttl", "auto"), None);
        assert_eq!(to_dash("context_window", "1m"), Some("1m".into()));
        assert_eq!(to_dash("events.auto_turn", "on"), Some("true".into()));
        assert_eq!(to_dash("startup.quick_start", "on"), Some("on".into()), "this one takes on/off as is");
        assert_eq!(to_dash("api_retries", " 5 "), Some("5".into()));
        assert_eq!(to_dash("api_retries", ""), None);
        assert_eq!(shown("cache_ttl", None), "auto");
        assert_eq!(shown("api_retries", None), "default");
        assert_eq!(shown("progressive_tool_disclosure", Some("false")), "off");
        for (key, _) in CONFIG_GROUPS.iter().flat_map(|(_, k)| k.iter().map(|k| (*k, ()))) {
            if let Some(opts) = config_choices(key) {
                for o in opts {
                    assert_eq!(shown(key, to_dash(key, o).as_deref()), *o, "{key} {o}");
                }
            }
        }
    }

    #[test]
    fn units_tags_and_first_sentences() {
        assert_eq!(
            (unit("bash_timeout"), unit("max_tool_output"), unit("daemon.idle_exit_secs"), unit("api_retries")),
            ("s", "bytes", "s", "")
        );
        assert_eq!(applies_tag("reload"), Some("AFTER RELOAD"));
        assert_eq!(applies_tag("live"), None);
        assert_eq!(first_sentence("One. Two."), "One.");
        assert_eq!(first_sentence("US dollars (decimal, e.g. 2.50) per turn. More."), "US dollars (decimal, e.g. 2.50) per turn.");
        assert_eq!(first_sentence("No stop"), "No stop");
        assert!(matches("ttl", "Cache TTL", "cache_ttl", ""));
        assert!(matches("spill", "x", "y", "saved whole to a spill file"));
        assert!(!matches("zzz", "Cache TTL", "cache_ttl", "help"));
    }

    #[test]
    fn config_lines_read_last_wins_and_models_check_as_the_dash() {
        let m = read_config("# c\nmodel = a/b\n\nmodel=c/d\nbad line\n turn_budget.worker.max_cost_usd = 2.50 \n");
        assert_eq!(m.get("model").map(String::as_str), Some("c/d"));
        assert_eq!(m.get("turn_budget.worker.max_cost_usd").map(String::as_str), Some("2.50"));
        assert_eq!(m.len(), 2);
        // L1: an allowlist of the TUI's rows; nothing else is held.
        let other = read_config("bridge.url = x\nshell.rc = y\nextension.foo.api_key = k\nextension.foo.secret = s\nmax_fps = 30\n");
        assert_eq!(other.into_iter().collect::<Vec<_>>(), [("max_fps".to_string(), "30".to_string())]);
        assert!(model_ok("anthropic/claude-opus-4-1") && model_ok("openai/gpt-5:latest"));
        assert!(!model_ok("claude") && !model_ok("a/") && !model_ok("a b/c"));
        assert!(read_config("auth.token = x\nprovider.openai.key = y\nmodel = a/b\n").len() == 1, "secrets never read");
    }

    #[test]
    fn typed_values_are_checked_before_sending() {
        assert_eq!(check("model", "a/b"), None);
        assert!(check("model", "claude").is_some());
        assert_eq!(check("subagent_timeout", "300"), None);
        assert!(check("subagent_timeout", "5m").is_some());
        assert_eq!(check("thinking_display", "omitted"), None);
        assert!(check("thinking_display", "loud").is_some());
        assert_eq!(check("model", "  "), None, "empty unsets");
        // L5: the dash's `^\d{1,9}$` within its range.
        for bad in ["2.5", "-1", "1e3", "NaN", "inf", "+3", "1234567890"] {
            assert!(check("api_retries", bad).is_some_and(|m| m.contains("whole number")), "{bad}");
        }
        assert_eq!(check("api_retries", "20"), None);
        assert_eq!(check("api_retries", "21").as_deref(), Some("api_retries: expected 0 to 20, got 21"));
        assert!(check("subagent_timeout", "9").is_some() && check("subagent_timeout", "86400").is_none());
        for (_, keys) in CONFIG_GROUPS {
            for k in keys.iter().filter(|k| def(k).is_some_and(|d| d.ctl == Ctl::Number)) {
                assert!(range(k).is_some(), "{k}: every CONFIG number has the dash's range");
            }
        }
    }

    /// The page's find of a row (settings_find.rs, PR 3; `matches` before).
    fn matches(find: &str, label: &str, key: &str, help: &str) -> bool {
        use crate::settings_find::{Fields, Query, find as found};
        found(&Query::parse(find), &Fields::new(label, key, help), &mut Vec::new()).is_some()
    }

    #[test]
    fn find_matches_any_case_without_allocating() {
        assert!(matches("", "x", "", ""));
        assert!(matches("api", "API retries", "api_retries", ""));
        assert!(matches("retr", "x", "API_RETRIES", ""));
        assert!(!matches("zzz", "API retries", "api_retries", "help"));
        assert!(matches("ü", "Grüße", "", ""));
    }

    #[test]
    fn an_unreadable_config_is_an_error_a_missing_one_defaults() {
        let base = std::env::temp_dir().join(format!("sd-ref-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        assert!(read_reference(&base, None).values.unwrap().is_empty(), "no file: every row default");
        std::fs::write(base.join("config"), "max_fps = 30\nauth.token = t\n").unwrap();
        assert_eq!(read_reference(&base, None).values.unwrap().len(), 1);
        // A profile's own file when it has one; the path read is the one shown.
        std::fs::create_dir_all(base.join("p")).unwrap();
        std::fs::write(base.join("p/config"), "api_retries = 4\n").unwrap();
        let r = read_reference(&base, Some("p"));
        assert!(Path::new(&r.path).ends_with("p/config"), "{}", r.path);
        assert_eq!(r.values.unwrap().get("api_retries").map(String::as_str), Some("4"));
        assert!(Path::new(&read_reference(&base, Some("q")).path).ends_with(format!("sd-ref-{}/config", std::process::id())));
        // A directory where the file should be: unreadable, not "default".
        std::fs::remove_file(base.join("config")).unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();
        assert!(read_reference(&base, None).values.is_err());
        std::fs::remove_dir_all(&base).ok();
    }
}
