# Q3 Life, round 2: evidence

The design gate (`cohesion/finish-gate-pr17.md`) and Shady's review of #17 at `8580aab`, answered. Recorded on bella
with `scripts/capture-linux.sh` (headless gamescope at 144 Hz, x11grab, `MESA_VK_WSI_DEBUG=sw`), driven by
`evidence2.sh` (here). Binary: this branch at `63e5804`, release build sha256 `d73d8a4b…`. The head after it, `f9be0a6`, changes only the
bench's `load_5x5_sweep_30s` case (plus docs).

## Clips (lossless: `libx264rgb -qp 0`, RGB; each file's per-frame md5 equals its raw capture's, `evidence2.sh`)

| file | level | size | fps | frames | length |
|---|---|---|---|---|---|
| `life-full-1920.mp4` | Full | 1920×1080 | 60 | 1546 | 25.8 s |
| `life-reduced-1920.mp4` | Reduced | 1920×1080 | 30 | 774 | 25.8 s |
| `life-off-1920.mp4` | Off | 1920×1080 | 30 | 773 | 25.8 s |
| `life-full-1280.mp4` | Full | 1280×720 | 30 | 774 | 25.8 s |

The scene is `SYNAPS_BENCH_LIFE=4` in `bench.rs`: after 4 s a turn runs a `bash` call, a scout spawns and works,
four workers join, `bash` ends ok, a `read` fails, the scout completes, seven probes spawn and then an eighth
(`+N more`), a prompt comes, a rail row is favourited, the prompt is answered and the turn ends. Every event is
stamped as a daemon's are.

## What the Full 1920 clip shows (frame numbers at 60 fps)

Per-frame pixel diffs (`diffs.py` boxes of what changed) back every line below.

| frames | what |
|---|---|
| 118–1157 | **The one sweep (P2):** the composer's top edge (y 948) sweeps from the turn's start until the prompt comes (NEEDS YOU outranks RUNNING). No other sweep appears in the clip: no row has one (the worker-row and bash-row sweeps are cut). `full-f580.png`: five worker rows and the bash row, each with its braille spinner only. |
| 386–401 (`full-tools-roll-f386-401-x2.png`, `full-step-roll-f386-401.png`) | **Transcript roll (P5):** scout's `1 tool` → `2 tools`. The digit cell (x 1111–1120) rolls over frames 389–400; the word swaps to `tools` in place at 388; nothing else in the row moves. The live line's step rolls too: the old text rises out and the new comes in from below (389–394). |
| 626–643 (`full-check-f626-643-x5.png`) | **Write-on (P4):** `bash` ends ok. The spinner's frame fades while the check draws over it. Every frame in the 24×22 gutter box has ink: at least 41 lit pixels (55 before, 58 after). |
| 766–779 (`full-cross-f762-779-x5.png`) | The failed `read`: the cross writes on over its spinner, at least 15 lit pixels in every frame (36 at rest). |
| 856–868 (`full-worker-done-f855-868-x5.png`) | The scout's row completes: the same, at least 33 lit pixels (50 at rest). |
| 1066–1078 (`full-more-roll-f1064-1079-x3.png`) | `+7 more` → `+8 more`: only the digit cell (x 1222–1230) changes. |
| 1156–1168 (`full-needs-roll-f1155-1169-x2.png`, `full-rail-chips-roll-f1155-1169-x2.png`) | The prompt: the top bar's NEEDS YOU digit rolls 0 → 1 (x 809–815; the word re-colours, it does not roll). The rail's NEEDS YOU and RUNNING chips roll their digits. Diffed against the frames before and after, the only pixels that change are inside the two digit cells (x 102–111 and 190–199), so the chips beside them stay still. |
| 1276–1305 (`full-star-f1284-1305-x5.png`) | **Star (P6):** the favourited row slides into FAVOURITES. Its title reaches rest (y 241) at 1292. Then the star rises 3 px (238 at 1294–1296), the four sparks show (1296–1302), and it settles at 241 by 1303. No hop or spark appears during the slide. |

**Reduced and Off** (`life-reduced-1920.mp4`, `life-off-1920.mp4`): no sweep, no roll, and no write-on: a glyph or
a count changes between two frames. In Reduced the clocks tick once a second; in Off they hold.

**1280×720** (`life-full-1280.mp4`): the same scene; the composer's sweep is at y 588.

## Not recorded: the settle pass (P3) live

`p3.sh` (here) drove the desktopproto sandbox. It checked first that no one was running the sandbox, started the
daemon itself, clicked + NEW, typed a message and sent it, then stopped the daemon. It recorded at 60 fps, Full then
Reduced. The turn never ran: the sandbox profile has no credentials. Its error is in `p3-sandbox-auth-error.png`:
`No credentials for anthropic at ~/.synaps-cli/auth.json`. The main profile authenticates through a broker, but the
sandbox's config has no broker settings. I didn't touch credentials, so P3 rests on its test:
`handoff_in_travels_and_goes`: the sweep while the turn runs; at a READY turn end, the pass from the same corner as
the bar's light; one pass, then nothing; none at Reduced or Off; no timer left after.
