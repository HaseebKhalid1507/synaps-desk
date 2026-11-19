# Synaps Desktop: GPUI spike report

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img), at the same paths. See [EVIDENCE.md](EVIDENCE.md).

Branch `spike/gpui`. Date 2026-11-13. Author: Spike (subagent), for Jawz / Haseeb.

**Bottom line: GO.** GPUI held the display refresh rate in every scenario, with
about 1 ms of CPU per frame. A warm launch reaches the first frame in about 0.3 s.
The thin-client layer (`synaps-client`) talks to a real 0.11.1 daemon, follows
`daemon reload` and builds on Windows and macOS in CI. The weak spots are memory, binary
size and upstream churn. Details and caveats follow.

## What was built

| Crate | What | Lines |
|---|---|---|
| `crates/synaps-client` | No GUI deps. Daemon discovery (`daemon[-profile].json`), Unix socket + Windows named pipe (pipe name derivation and same-user server check ported from Synaps `ipc.rs`, attributed), tolerant serde mirrors of the v3 frames, handshaken connection with 64 MiB frame cap, a connection thread (control connection + one attached session, reload reconnect with `Hello{reconnect_of}`), a `Transcript` model (history + live events → display rows, with splice-shaped change tracking), a deterministic synthetic load generator, fixture tests, and the `synaps-probe` headless bin. | ~2,450 |
| `crates/synaps-desktop` | The GPUI app: session rail, virtualized transcript (GPUI `list` + `ListState`), streaming markdown with highlighted code blocks, collapsible thinking, compact tool rows, composer (Enter sends, Shift+Enter newline, Abort while streaming; typing while busy steers), connection pill. `--bench` / `--diag` self-driving measurement modes. | ~1,240 |

Screenshots (bench mode, bella, dark theme): `docs/img/streaming.png`,
`docs/img/long-transcript.png`, `docs/img/daemon-turn.png`.

The public API of `synaps-client` is documented at the top of its `lib.rs`:
`Client::connect(profile, Identity::desktop())` gives `(Client, Receiver<Event>)`.
The client has `list_sessions / attach / create / submit / abort / detach`.
*(After review, PR #2, the API changed; see `crates/synaps-client/CHANGELOG.md`.)*

## Machine

Window launches on Haseeb's display: 7 (1 bench under load, 1 `--diag`, 3 official, 1 screenshot run, 1 confirmation
after the toolchain bump). Every launch was `--bench`/`--diag`, auto-exited in 15–27 s and ran inside
`systemd-run --user --scope -p MemoryMax=2G -p MemorySwapMax=0`.

bella: Intel Core Ultra 9 275HX (24 threads), 30 GiB RAM, NVIDIA GeForce RTX 5070 Ti Laptop
(open kernel module 615.78.08, selected by GPUI over Vulkan; Intel Arrow Lake iGPU and llvmpipe
also enumerated), CachyOS kernel 7.3.0, **X11** (Xorg) + **i3 4.25.1** + **picom v13** compositor.
The window opened on DP-3, 2560×1440 at **143.86 Hz**, RandR scale 1.083. The window type was
`dialog` in bench mode, so i3 floated it and did not re-tile Haseeb's workspace. The script
checked that the window was floating on every run. The screen was unlocked and DPMS was
disabled, so the frame rate was not throttled. GPUI did not report the refresh interval on
X11, so the period comes from `xrandr` (`--refresh-hz 143.86`).

Other sessions were building on bella. Before each run the bench waited for a 1-minute load
under 4. The recorded load was 3.9 / 3.3 / 2.7 before the three runs, with no `rustc`
running. Two earlier runs are not in the table. Launch 1 ran under load 13, while other
builds were running, kswapd was active and the NVIDIA shader cache was probably cold. It got 9.8 s to
the first frame and 36–50 fps. That is what a first launch on a busy, swapping machine can
look like. It is not the steady state. Launch 2 was a `--diag` run used to split costs.

## Numbers (median of 3 `--bench` runs, raw values after)

Release build (`lto = "thin"`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, `strip = true`), built with Rust 1.95.0.
The toolchain is now pinned to 1.100.0 for macOS (see CI). One confirmation run with the 1.100.0 build matched these numbers:
stream 143.1 fps, 5 dropped, draw p50/p99 1.04/2.89 ms; scroll 144.0 fps, 0 dropped, draw p50/p99 0.83/2.24 ms
(`docs/bench/*-rust1.100-confirm.json`). The raw JSON for every run is in `docs/bench/`.
Frame data comes from GPUI's own profiler (`gpui/profiler`: `FrameEvent::Draw`/`Present`).
`draw` is the CPU time inside `Window::draw`, covering layout, prepaint and paint.
`interval` is the time between consecutive presents. `dropped` is the sum of
`round(interval / refresh) − 1`. The bench calls `request_animation_frame` in every
frame, so an idle window still presents at the full rate.

### Startup

| | median | runs |
|---|---|---|
| launch (`date` before exec) → first frame presented | **311 ms** | 630 / 311 / 309 |
| `main()` → first frame presented | 295 ms | 617 / 295 / 292 |
| of which: platform up (X11, D-Bus portal) | ~17 ms | `app_run_callback` mark |
| of which: wgpu adapter enumerate + device + surface test (NVIDIA Vulkan) | ~270 ms | log timestamps (launch 2: 20 ms → 288 ms) |

Most of the startup time goes to wgpu probing all three adapters, including a GL/EGL probe
on NVIDIA that prints `MESA-EGL` warnings, and then configuring the NVIDIA Vulkan device. App code is negligible.

### (a) Streaming synthetic markdown, ~200 deltas/s, into a 40-message transcript (5 s, 9,037 chars, code blocks)

| | median | runs |
|---|---|---|
| fps (display 143.86 Hz) | **143.5** | 143.1 / 143.5 / 143.5 |
| dropped frames (of 733 refreshes) | **6** | 6 / 6 / 6 |
| draw p50 / p95 / p99 / max (ms) | **1.09 / 1.58 / 2.70** / 12.4 | p99: 2.94 / 2.68 / 2.70 |
| frame interval p50 / p95 / p99 / max (ms) | **6.96 / 7.22 / 8.62** / 18.8 | p99: 8.62 / 8.63 / 8.56 |
| deltas/s achieved / received | 200.2 / 1000 of 1000 | |

The 6 dropped frames are the same in every run. They are probably the large markdown
re-layouts when a new code block opens; the max draw is 12 ms. Deltas are folded into the
transcript **once per frame**. The pump buffers them and notifies only on the first one
after a frame, and `render` applies the batch. Only the growing row is spliced and re-measured.

### (b) Scrolling a synthetic 2,000-message transcript (2,197 rows: markdown, code blocks, thinking, tool rows) at 4,000 px/s for 6 s

| | median | runs |
|---|---|---|
| fps | **144.0** | 144.0 / 144.0 / 144.0 |
| dropped frames (of 863 refreshes) | **0** | 0 / 0 / 0 |
| draw p50 / p95 / p99 / max (ms) | **0.82 / 1.76 / 2.10** / 2.8 | p99: 2.05 / 2.35 / 2.10 |
| frame interval p50 / p95 / p99 / max (ms) | **6.95 / 7.90 / 8.25** / 9.0 | |
| rows travelled | 0 → 178 (~24,000 px) | |

Each row renders and parses its markdown only while it is visible. Row state is GPUI keyed
element state, which is dropped once a row leaves the overdraw window. The `--diag` run split
the cost. Static 2,000-row transcript: draw p50 0.62 ms. Scrolling with plain-text rows:
0.49 ms. Scrolling with markdown rows: 0.77 ms. Static empty window: 0.26 ms.

### Memory (`/proc/self/smaps`, cross-checked against `smaps_rollup`; MiB)

"GPU maps" are the mappings of `/dev/dri`, `/dev/nvidia*`, `libnvidia*` and the Vulkan
loader and ICDs. That is mostly the NVIDIA user-space driver.

| point | RSS | PSS | USS | GPU maps RSS / PSS | USS excl. GPU maps |
|---|---|---|---|---|---|
| idle, 1.5 s after first frame | **193** | **172** | **158** | 93 / 78 | **93** |
| after streaming | 241 | 219 | 205 | 95 / 80 | 138 |
| 2,000-message transcript loaded | 243 | 221 | 207 | 95 / 80 | 140 |
| after scrolling it | **244** | **223** | **208** | 95 / 80 | 141 |

Runs (RSS idle): 247 / 193 / 193. The idle number is bimodal. The **first launch of a freshly built binary** maps
about 38 MiB more NVIDIA driver memory (131 vs 93 MiB). Later launches do not. The 1.100.0 confirmation run, also
a first launch, shows the same 247 MiB. This is most likely the driver's per-binary shader/pipeline cache being built. The 2,000-message transcript adds about 2 MiB, which shows virtualization works.
The +46 MiB step comes with the first streamed markdown. It is not broken down further. The likely
causes are the syntect syntax set (loaded lazily on the first code block), the glyph atlas and
the line-layout caches. Without the NVIDIA driver mappings the app is about 93 MiB idle.
That is not tiny. For comparison, the sandbox daemon on bella was 39 MiB RSS and an idle
default-profile daemon 17 MiB, and a typical Electron chat app is 300–500 MiB.

### Real turn against the sandbox daemon (`desktopproto`, claude-haiku-4-5, thinking low, prompt "Reply with one short sentence about Rust.")

| | median | runs |
|---|---|---|
| create + attach (round trip) | 5.7 ms | 18.0 / 5.5 / 5.7 |
| time to first text token (received on the UI thread) | **1,101 ms** | 1,101 / 1,324 / 943 |
| …until that token was on screen | **1,109 ms** | 1,109 / 1,327 / 950 |
| full round trip to `Idle` | **1,271 ms** | 1,271 / 1,464 / 1,103 |

Almost all of that time is the model. Client overhead from receiving a token to painting it is
2–7 ms. The headless probe got the same order: TTFT 1,584 ms, first delta (thinking)
766 ms, turn 1,870 ms. The probe also followed a live `synaps daemon reload`: Reloading,
then `Bye`, then a reconnect with `reconnect_of`, then re-attach with the full history.
This happened with no user action.

### Binary size and build times (bella, shared build lane)

| | |
|---|---|
| `synaps-desktop` Linux x86_64, stripped | **34,057,704 bytes (32.5 MiB)** with Rust 1.95; 34,004,488 with 1.100. A hello-world with the same gpui-kit features: 31 MiB |
| `synaps-desktop.exe` Windows x86_64 (CI) | **17,469,440 bytes (16.7 MiB)** (no X11+Wayland+Vulkan/GL stack; uses DirectX) |
| `synaps-probe` Linux / Windows | 1,099,136 / 908,288 bytes |
| cold release build (all 536 crates), quiet bella, 10 jobs | **1 m 29 s** (toolchain switch to 1.100.0), peak 5.7 GiB |
| cold release build under load (avg ~13, other sessions' builds) | 7 m 57 s in the lane, plus 4 m queued |
| warm release rebuild, app crate edit, quiet box | **24 s** (thin LTO + 1 CGU relink); 3.5–4.5 min when other builds were saturating bella |
| CI cold release build (GitHub runners) | ubuntu 15 m 01 s, windows 13 m 19 s |
| target dir on bella | 3.7 GiB (includes stale 1.95 artifacts from before the toolchain bump) |

Ways to cut the size, not tried: drop `syntect` for a smaller highlighter (it carries bat's
syntax set), drop the default icon `assets`, use `opt-level = "s"`, use fat LTO. The size
does not affect frame time.

## CI

`.github/workflows/ci.yml`: `ubuntu-latest` and `windows-latest` run on push and PR. Each runs
`cargo test -p synaps-client`, a release build of both crates, and uploads the binaries as
artifacts. `macos-latest` runs only on `workflow_dispatch` or tags. Swatinem/rust-cache is enabled.

| OS | result | release build (cold) | `synaps-desktop` | notes |
|---|---|---|---|---|
| ubuntu-latest | **green** ([run 43391715707](https://github.com/HaseebKhalid1507/synaps-desktop/actions/runs/43391715707), Rust 1.95; green again on 1.100.0 in [run 43395128132](https://github.com/HaseebKhalid1507/synaps-desktop/actions/runs/43395128132)) | 15 m 01 s | 34,057,584 B | client tests 10/10 (incl. doctest) |
| windows-latest | **green** (both runs) | 13 m 19 s | 17,469,440 B (`.exe`) | client tests 13/13 (incl. doctest), including the named-pipe name derivation and connect-without-server tests; GPUI's DirectX backend compiles |
| macos-latest (arm64), `workflow_dispatch` | first run **red**: `gpui-pre-macos` needs Rust ≥ 1.98 (`Option::map_or_default`); after pinning 1.100.0 **green** ([run 43395134179](https://github.com/HaseebKhalid1507/synaps-desktop/actions/runs/43395134179)) | 2 m 22 s | 14,852,000 B | client tests pass; no runtime check (no Mac) |

Windows and macOS are **compile + unit-test only**. Nobody has opened the window on those platforms yet.

## What gpui-component gave us vs what we wrote

Free (worked on the first try, with no custom code):
- **Markdown rendering** (`TextView::markdown(id, text)`), incremental: `set_text` that extends
  the old text is appended and re-parsed incrementally, not from scratch. Inline code, lists and tables work.
- **Highlighted fenced code blocks** with the `syntect` feature, plus a language label.
- **Theming**: dark mode in one call, a full token set (`theme.success`, `sidebar`, `muted`…).
- **Multi-line composer** (`TextareaState::auto_grow(1, 8).submit_on_enter(true)`, Shift+Enter = newline).
- **Buttons**, scrollable containers, the `Root` that hosts dialogs and notifications.
- **Frame profiler** (`gpui/profiler`): draw phases, present timing, `FrameTimingCollector`.
- Not used yet but available: `MessageScroller` (a chat list with tail-follow and a jump-to-latest
  button), message/bubble components, sidebar, notifications and dialogs.

Written by us:
- The whole wire layer, the transcript reducer, and reload/reconnect.
- The per-frame coalescing of stream events, and splice-based updates to GPUI's `ListState`, so
  only changed rows are re-measured.
- Tool rows, collapsible thinking, the session rail and the status pill (plain `div`s, a few dozen lines each).
- The bench harness: smaps memory split, frame statistics, synthetic load.

For the virtual list we used GPUI's own `list` + `ListState` (variable-height rows,
`FollowMode::Tail`), not gpui-component's `v_virtual_list`. It handles rows of variable height
and the tail-following a chat needs, and the bench needs `scroll_by`.

## GPUI API pain points and churn risk

- **Supply chain.** Zed stopped publishing `gpui` after 0.2.2. Longbridge republishes Zed's
  GPUI as weekly `gpui-pre-*` snapshots, and gpui-kit pins one exactly (`=0.3.12`). Each
  gpui-kit bump means a new GPUI snapshot, with API changes we have to absorb. Their own
  issue #3156 was apps breaking on a caret pin. Some of gpui-kit's own deps are caret
  (`gpui-base ^0.9.1`, `gpui-component ^0.9.1`), so **our `Cargo.lock` is the real pin**,
  and upgrades should be deliberate. The project depends on Longbridge staying active
  (it is active now: weekly releases, about 20k stars).
- **The facade hides GPUI paths.** You `use gpui_kit::*`. Builder helpers like `.when()` need
  `gpui_kit::prelude::FluentBuilder`, which costs a compile round to find. Some docs and
  examples still use `gpui::` paths.
- **Borrowing.** `cx.theme()` borrows the `Context`, so helpers that need a `&mut Context`
  conflict with it. The fix is to write render helpers against `&Context<Self>`. This is
  learnable, not a blocker.
- **Linux fonts.** `.SystemUIFont` fails and falls back to Adwaita Sans (logged as a warning).
  The generic family `"monospace"` does **not** resolve; use `theme.mono_font_family`.
- **Throttling defaults.** Windows get `inactive_frame_interval ≈ 30 fps` when unfocused, plus
  low-power and thermal caps. A benchmark or an always-on dashboard must turn these off explicitly.
- **wgpu startup** probes every backend, GL included, and prints `MESA-EGL` warnings on NVIDIA.
  It costs about 270 ms. `ZED_DEVICE_ID` can pin an adapter, but there is no API to skip the GL probe.
- **Frame timing on X11**: `FrameTiming.refresh_interval` is `None`, so you have to supply the panel rate yourself.
- `activate()` is not implemented on Linux (logged). Not a problem for a client app.
- Build cost is fine: cold 1.5 min on a quiet 24-thread box (about 6 GiB peak), warm 24 s; 13–15 min cold on GitHub runners.
- **MSRV moves with the snapshot.** `gpui-pre-macos 0.3.12` calls `Option::map_or_default` (stable since
  Rust 1.98). Linux and Windows built fine on 1.95.0, and only the first macOS CI run caught it. We now pin 1.100.0.
  Every gpui-kit bump can raise the MSRV, so run macOS CI on each bump.

## Wayland / X11 / IME

- Measured on **X11 only** (bella runs i3 + picom). GPUI is built with both the `x11` and
  `wayland` backends, and Wayland was **not exercised** in this spike. GPUI picks the backend
  at runtime from `WAYLAND_DISPLAY`. A Wayland session (KDE or GNOME) needs a separate run before release.
- The hybrid-GPU logic picked the discrete NVIDIA GPU. It uses the compositor GPU hint, so it
  stays on the GPU the X server renders on.
- **IME was not tested** (no human at the keyboard). GPUI has XIM on X11 and text-input-v3 on
  Wayland, and Zed ships with them, but CJK input needs a manual check.
- Fractional scale 1.083 (RandR) was picked up correctly.

## Protocol gaps a full client needs

None of these blocked the spike. Items 1, 2, 3, 4, 7 and 8 need a Synaps change to do properly; item 5 turned out not to be a gap.
1. **No `desktop` client kind.** `ClientKind` is a closed enum (tui/chat/attach/rpc/server/test),
   and an older daemon rejects an unknown kind as a malformed frame. We send `kind: "attach"`
   (interactive, so keep-warm treats it like the TUI) with `name: "desktop"`. A proper kind
   needs a Synaps change and a `features` gate.
2. **Tool results have no structured status or exit code.** The stream sends
   `ToolResult{tool_id, result}` as text only. History `tool_result` blocks sometimes carry
   `is_error`. A "name + status + exit" row needs `is_error` and `exit_code` on the wire.
   Today the row shows done or failed, and the exit code is not shown.
3. **The session list is not pushed.** Clients poll `Sessions` on a control connection
   (every 3 s in the desktop app; opt-in in `synaps-client` since PR #2). A
   `sessions_changed` subscription would remove the polling.
4. **History arrives all at once.** `Attached` ships the whole `api_messages` (up to 64 MiB),
   provider-shaped, and the client re-derives the display rows. `Digest` mode's
   `display_tail` only covers the tail. Long sessions need paginated, render-ready history
   ("give me items before X") for fast attach and scroll-back.
5. **cwd per session.** *(Corrected after review.)* The daemon fills `SessionConfig.cwd`
   from `Hello.cwd` only when the create leaves it `None` (`daemon/conn.rs`, `if
   config.cwd.is_none()`), and refuses a relative or missing directory. So
   `CreateConfig.cwd` works today: `synaps-client` now always sends one
   (`CreateOptions::cwd`, default the user's home), so a GUI started from a launcher no
   longer runs its sessions' tools in `/` or wherever it was launched. Not a gap.
6. **Environment.** `Hello.env` is `None` here, so created sessions get the daemon's
   environment. The TUI sends a secret-stripped snapshot of its shell. A GUI launched from
   a desktop launcher has no useful shell environment, so we need a policy (probably keep
   `None`, or use a configured env).
7. **Permission prompts.** `Prompt` and `Answer` exist, but `PromptKind`'s values are not
   documented for third-party clients. The spike shows pending prompts read-only.
8. **No "recent sessions" over the wire.** `Sessions` lists sessions the daemon holds, live or
   parked. Saved sessions it does not hold are only reachable with `Attach::Create{continue_session}`
   if you already know the id. The rail can show live sessions but not recent history without a
   journal-listing query (or reading `~/.synaps-cli/sessions` directly, which is not thin).
9. **Daemon lifecycle and auth are outside the wire.** Starting a daemon, `synaps login`
   (OAuth) and listing models have no frames. The desktop would shell out to the `synaps`
   binary, or Synaps adds control frames. This decides packaging: bundle or require `synaps`.

## Recommendation: **GO**, with conditions

Why go:
- **Fast.** The display rate held in both stress tests, at 143.86 Hz, with about 1 ms of CPU
  per frame (p99 under 3 ms). There is a lot of headroom for real features. A token reaches
  the screen 2–7 ms after it arrives.
- **Cross-platform for real.** The same code builds for Linux (X11 and Wayland compiled in) and
  Windows (DirectX backend, plus our named pipe tested in CI). macOS is checked once by
  `workflow_dispatch` (see CI). The client layer is plain Rust and tokio.
- **Modern look for little code.** The screenshots come from about 500 lines of view code,
  thanks to gpui-component's markdown, highlighting, theming and textarea.
- **The thin client works**: real daemon, real turn, reload reconnect, tolerant decoding,
  and no engine dependency.

Conditions and risks to accept:
- **Memory is moderate, not light**: about 190 MiB RSS idle on NVIDIA, of which about 93 MiB
  is driver mappings. That is about 245 MiB with a long transcript after streaming. That
  beats Electron but is roughly 5× the daemon. Measure on Intel/AMD and macOS before promising numbers.
- **Upstream churn**: budget a day per gpui-kit upgrade, keep `Cargo.lock` as the pin, and
  upgrade on our schedule. If Longbridge stops, the fallback is vendoring a snapshot (Apache-2.0).
- **Untested so far**: Wayland, IME, Windows and macOS at runtime (CI only compiles and runs
  unit tests there), HiDPI on macOS, and accessibility.
- **First launch on a busy machine** with a cold NVIDIA shader cache was 9.8 s (launch 1). On a
  quiet machine with warm caches it is 0.3 s. Packaging should warm the shader cache, or
  accept that the first launch is slow.
