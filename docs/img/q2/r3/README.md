# Q2 r3: live on the desktopproto sandbox, the finish gate's states, lossless clips

## Binaries

| build | sha256 | what came from it |
|---|---|---|
| `1e640db` | `13fa1261…` | run A, reduced, off, paper, 860×640, the 640×420 shots, the offline r3 stills |
| `2fd6ab1` | `0e8158da…` | run B (Allow all, after the `took` fix below) |
| `a02a930` | `66a9ab19…` | run T (the trace, after the dashes fix below); the memory bench (`docs/bench/q2/r3`) |
| `004cd5e` (HEAD) | `9c07579c…` | installed as `bella:~/.local/bin/synaps-desktop-hybrid.q2` |

From `1e640db` to `a02a930` only two things changed: the receipt's `took` and the dashes' timing. Both were
found in these runs. `004cd5e` moves the read gate's arithmetic into a tested function and changes no
behaviour.

## The sandbox

- **Daemon:** `synaps daemon --profile desktopproto` on bella, started by this run (pid 1914951). It was not
  running before. Model `claude-haiku-4-5`, with the guard extension.
- **Auth:** two `auth.` lines were added to `~/.synaps-cli/desktopproto/config` for the model calls. Afterwards
  the daemon was stopped and the config restored from its copy. The restored file's sha256 starts `c1b7ac41`,
  the same as before, and it has 0 `auth.` lines.
- **Capture:** the app ran in a headless gamescope through `scripts/capture-linux.sh`: x11grab at 30 fps,
  `libx264rgb -qp 0`, `gbrp`, `MESA_VK_WSI_DEBUG=sw`, `CAPTURE_HOVER=0`. It was driven by xdotool. Every
  step's time from t0 is in `drive-<run>.log`.

## Runs (1280×720 unless named)

| run | motion | theme | what it does (see `drive-<run>.log`) |
|---|---|---|---|
| A | Full | nord | confirm: hover DENY / ALLOW, press both (released off), Ctrl+Enter · confirm: Esc (denied) · a new session: the TUI attaches with `--takeover` and answers `y` · confirm: 14 wheel notches up (stub), Ctrl+J, Ctrl+Enter · confirm: click Allow all, Ctrl+Enter · `sudo -k true`: secret card, Ctrl+Enter twice on the empty field, Esc |
| B | Full | nord | confirm: click Allow all, Ctrl+Enter (`2fd6ab1`) |
| T | Full | nord | a confirm arrives (`a02a930`) |
| reduced, off | Reduced / Off | nord | confirm: Ctrl+Enter · confirm: Esc |
| paper | Full | paper | confirm: hover, press, click Allow all, Ctrl+Enter |
| n860x640 | Full | nord | 860×640: confirm, Ctrl+Enter |

`scripts/capture-linux.sh` at 640×420 panics in wgpu (`Surface is not configured for presentation`). Main's
build panics the same way, so the 640×420 shots (`live-640x420-*.png`) were taken differently: the same
binary, live on the sandbox, `--motion off`, and `import -window` between steps. The steps were a confirm,
Esc, 6 wheel notches up, Ctrl+J, Ctrl+Enter.

## Clips: all lossless

The `.mp4` files are remuxed (`-c copy`) from `.mkv` cuts. Each cut was encoded with
`libx264rgb -qp 0 -preset veryslow`; its x264 header reads `rc=cqp … qp=0`, and the pixel format is `gbrp`.
For every clip, the frame-by-frame md5s of the `.mp4` are identical to the raw capture's frames over the same
span. `verify.txt` lists each clip with its frame count and both hashes.

| clip | from | shows |
|---|---|---|
| `full-1280-trace.mp4` | A 5.5–9.5 s | the frame traces (frames 24–30); **every dash lands in frame 31** (`trace-1e640db-f*.png`) |
| `full-1280-trace-a02a930.mp4` | T 5.5–9.5 s | the fix: the dashes follow the top stroke's head. In the raw capture, row y=301 has 89 dash px in frame 78, 245 in frame 79 and 418 (all) in frame 80 (`trace-a02a930-f77…f81.png`) |
| `full-1280-hover-press.mp4` | A 15.5–21.2 s | hover DENY, hover ALLOW, press DENY, press ALLOW, each released off the button |
| `full-1280-allow-collapse.mp4` | A 20.8–24.8 s | Ctrl+Enter; `answer sent, waiting for the daemon` on one line; the collapse into the row's receipt |
| `full-1280-deny-collapse.mp4` | A 39.5–43.5 s | Esc; the collapse; `denied … · 9s` |
| `full-1280-answered-elsewhere.mp4` | A 60.5–64.5 s | the TUI answers `y`; `answered elsewhere` |
| `full-1280-ctrlj-from-stub.mp4` | A 78.3–88 s | the stub; Ctrl+J (82.14 s); the glide (82.67–82.87 s); the keys arm (83.37 s); Ctrl+Enter (84.67 s); the collapse from 85.2 s |
| `full-1280-allow-all.mp4` | A 104.5–110 s | Allow all clicked, ALLOW; the receipt read `allowed all … · 1s`, the bug fixed in `2fd6ab1` |
| `full-1280-secret.mp4` | A 127.5–143 s | the `sudo` secret card; Ctrl+Enter on the empty field sends nothing; Esc cancels |
| `reduced-1280-*.mp4`, `off-1280-*.mp4` | reduced, off | the arrival (no trace), allow, deny: the row goes at once |
| `paper-full-1280-hover-press-allowall.mp4` | paper | hover, press, Allow all checked, ALLOW |
| `full-860x640-card-allow.mp4` | n860x640 | the card at 860×640, ALLOW |

**The collapse (`full-1280-allow-collapse.mp4`, frames 31–39):** the card's frame top holds at y=300, under its
call row, while its bottom closes up. Above y=300 only these change during the collapse: the call row's meta
(the receipt, frame 31, rows 261–281), the gutter glyph, and the top bar. No row above moves. The card never
reaches the composer. In the composer's band (y ≥ 590), two things change: its status word (`NEEDS YOU` →
`RUNNING`, frame 30, before the collapse) and, in frames 32–33, the lower edge of a rail tooltip
(x 323–619). That tooltip covers the card's left half in frames 30–33. The drive parks the pointer on the
rail, so the tooltip is the drive's doing, not the app's.
Reduced and Off take the row out in one frame: `reduced` frame 33 and `off` frame 37 (frame numbers are
each clip's own). The call row's tick and receipt follow in the next frame.

## Stills (full frames from the captures)

| file | state |
|---|---|
| `card-armed-nord.png`, `card-armed-paper.png` | armed: ALLOW filled with `ctrl ↵`, DENY with `esc`, the Allow all line in mono `dim_k`; the command is the loudest text |
| `card-hover-{deny,allow}-{nord,paper}.png`, `card-press-{deny,allow}-{nord,paper}.png` | hover, press |
| `card-allow-all-checked-{nord,paper}.png` | Allow all checked: the box filled, the text in `prose_k` |
| `card-not-input-owner-nord.png` | the TUI owns the input: no Allow all line (DENY / ALLOW only) |
| `receipt-allowed-nord.png` | `allowed 19:14 · 15s` |
| `receipt-denied-nord.png` | `denied 19:14 · 9s` (`denied` in `err`) |
| `receipt-answered-elsewhere-nord.png` | `answered elsewhere 19:14` |
| `receipt-allowed-all-nord.png` | `allowed all 19:30 · 13s` (run B) |
| `secret-card-nord.png`, `receipt-secret-cancelled-nord.png` | the `sudo` secret card; `secret · cancelled 19:16 · 5s` |
| `ctrlj-0-stub-nord.png`, `ctrlj-1-landed-nord.png` | the stub; after Ctrl+J, the card in view with its buttons |
| `card-860x640-nord.png`, `receipt-860x640-nord.png` | 860×640 |
| `live-640x420-{1-arrived,2-card,3-stub,4-ctrlj,5-receipt}.png` | 640×420: the actions wrap onto their own line, and ALLOW stays on the card |
| `offline-{nord,paper}-1280-1-card.png` | offline demo, `1e640db` |
| `offline-{nord,paper}-{1280,860}-secret-2-secret.png` | the secret card at 1280×720 and 860×640: the status line has its own row and reads in full (`offline: nothing to answer`) |

The live owner case lasted from 53 s to 61 s in run A. The TUI took the input with `--takeover`, the card
dropped its Allow all line, and the TUI answered.

## Not shown

- **`secret · provided`:** not shown. It needs the account's real sudo password, and a wrong one counts
  toward faillock, so the secret was cancelled instead. The receipt's words are in a unit test
  (`a_receipt_says_what_and_when`).
- **The daemon's refusal of Allow all from a non-owner:** not exercised live. The desktop does not offer
  the line to a non-owner (`card-not-input-owner-nord.png`), and it sends `"always"` only if both the scope
  and the ownership still hold at the click (`allow_all_is_the_input_owners`).

`tui-pane-answered-elsewhere.txt` is the TUI pane as it showed the same prompt.
`roles_check.txt` is `cohesion/roles_check.py` run on this tree (Q2 floors included).
