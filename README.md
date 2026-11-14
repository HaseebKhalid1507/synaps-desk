# Synaps Desktop

Native desktop client for the [Synaps](https://github.com/HaseebKhalid1507/SynapsCLI) daemon.
Windows, macOS and Linux. Built on GPUI (via Longbridge's gpui-kit).

A thin client: it speaks the daemon's wire protocol over the local socket
(Unix domain socket; named pipe on Windows) and never links the engine.

Status: spike. See [docs/spike-report.md](docs/spike-report.md).

```text
crates/synaps-client    wire client (no GUI): discovery, socket/pipe, protocol, transcript, synaps-probe
crates/synaps-desktop   the GPUI app (bin synaps-desktop; --bench / --diag measurement modes)
scripts/                bench-linux.sh (run one memory-capped bench), bench-summary.py,
                        gen-palettes.py (the Synaps TUI's 20 palettes -> src/palettes.rs; --check)
```

```sh
synaps-desktop --profile <profile>      # connect to ~/.synaps-cli/run/daemon-<profile>.sock
synaps-probe --profile <profile> --prompt "hi"   # headless: list, create, stream, timings
synaps-desktop --theme tokyo-night --accent '#e8132f'   # any of the TUI's 20 themes; optional accent
```

Colour comes from the Synaps TUI's theme roles: the theme named by `--theme`, `$SYNAPS_THEME`,
the in-app picker (THEME, top right), `theme =` in the Synaps config, else nord. The accent is an
optional override of the "you" and focus colours only. Ctrl+= / Ctrl+- / Ctrl+0 (Cmd on macOS)
step the centre pane's text from 14 to 22 px.
