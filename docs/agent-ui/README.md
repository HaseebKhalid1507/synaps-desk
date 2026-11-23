# Native agent UI — experimental foundation

Run `synaps-desktop --agent-surface docs/agent-ui/plan.json`. The flag is hidden
(not in `-h`) and reads a local file you name: there is no agent source yet.
This isolated, read-only preview does not connect to the daemon or alter settings.
No changes to SynapsCLI or JavaScript/webview dependencies are required.

This implements a strict subset of A2UI 1.0 `createSurface` with a custom
`synaps-desk:preview/1` catalog: literal Text, Column and Row only. It is **not**
the Basic Catalog or a complete A2UI renderer. Unknown catalogs/components and
fields (A2UI's `@` envelope keys included), a field the component's kind does
not take, and a key given twice fail closed, at every level. The surface's and
every component's id is `[A-Za-z0-9._-]`, 1 to 128 bytes. Payloads are capped at
64 KiB, trees at 128 nodes/16 levels (the root's included), and text at 8 KiB.
Shared nodes/cycles, missing children and unreachable nodes are rejected. No
functions, URLs, generated code, permission actions or secrets.

Follow-up integration gates: editable controls and focus/keyboard UI tests,
incremental updates and reconnect semantics, session-scoped delivery, text
fallback, host-approved action dispatch, and visual/accessibility verification.
The current daemon protocol is unchanged; this preview does not claim live agent
integration. UI SDK's DOM adapters cannot render GPUI; the planned native bridge
borrows its state/action model, not its npm packages.

Protocol reference: https://a2ui.org/specification/v1.0-a2ui/
