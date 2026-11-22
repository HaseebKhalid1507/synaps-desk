# Native agent UI — experimental foundation

Run `synaps-desktop --agent-surface docs/agent-ui/plan.json`.
This isolated, read-only preview does not connect to the daemon or alter settings.
No changes to SynapsCLI or JavaScript/webview dependencies are required.

This implements a deliberately restricted A2UI 1.0 `createSurface` with a custom
`synaps-desk:preview/1` catalog: literal Text, Column and Row only. It is **not**
the Basic Catalog or a complete A2UI renderer. Unknown catalogs/components and
fields fail closed. Payloads are capped at 64 KiB, trees at 128 nodes/16 levels,
and text at 8 KiB. Shared nodes/cycles, missing children and unreachable nodes
are rejected. No functions, URLs, generated code, permission actions or secrets.

Follow-up integration gates: editable controls and focus/keyboard UI tests,
incremental updates and reconnect semantics, session-scoped delivery, text
fallback, host-approved action dispatch, and visual/accessibility verification.
The current daemon protocol is unchanged; this preview does not claim live agent
integration. UI SDK's DOM adapters cannot render GPUI; the planned native bridge
borrows its state/action model, not its npm packages.

Protocol reference: https://a2ui.org/specification/v1.0-a2ui/

## Native semantic action bridge contract

`--agent-action-check docs/agent-ui/action.json` exercises a local, non-executing
snapshot/authorization round trip. The host supplies the action allowlist and
validates action arguments. Each accepted action consumes its revision before
any future dispatch, rejecting repeat invocations. Session, surface and revision
must all match; snapshots expose only capability names, not arbitrary app state.
Requests are limited to 8 KiB. The fixture accepts only selection feedback.

This is a bridge **foundation**, not UI SDK compatibility or a live endpoint.
The CLI check has fresh state on each launch and executes nothing; it is not a
persistent anti-replay service. Before production, the desk must own bridge
lifetime, invalidate generations on reconnect, and wire both human and agent
interactions to one handler. Never expose permission approvals. Transport,
editable controls and state synchronization remain follow-up work.
