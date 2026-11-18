# Q6 palette: bench (gates 3 and 5)

bella, headless gamescope at 144 Hz, 1920×1080, `--bench-motion --refresh-hz 144`. Main is `0a78bb7` and Q6 is
`20f3017f…` (installed as `synaps-desktop-hybrid.q6`). Both are release builds made the same way (`dbuild … build
--release --locked -p synaps-desktop`). Three runs each, alternating main / Q6 (`bench.sh`), each run waiting for a
load under 6. Every run's config is a scratch folder under `~/.cache/q6` (an absolute `XDG_CONFIG_HOME`). The real
`~/.config/synaps-desktop/settings.json` kept its mtime (00:54:01, before the first run) through every run.

The Q7 worker's `--bench-motion` runs were on bella at the same time (a gamescope of theirs was seen during this
series). That affects CPU time, not presents or per-process memory.

Raw: `{main,q6}-motion-{1,2,3}.json`. Table: `summary.md` (`sum.py`).

| run | idle_30s_prompt presents / app frames / layer paints / cpu ms/s | pending, demoted | mem_idle anon / USS | after 5×5 anon / USS | mem_end anon / USS |
|---|---|---|---|---|---|
| main-1 | 0 / 0 / 0 / 2.2 | yes, yes | 30,368 / 57,180 | 58,940 / 89,328 | 64,340 / 94,272 |
| q6-1 | 0 / 0 / 0 / 1.94 | yes, yes | 30,236 / 61,872 | 58,856 / 92,156 | 64,956 / 98,596 |
| main-2 | 0 / 0 / 0 / 2.25 | yes, yes | 30,168 / 56,864 | 58,500 / 88,284 | 64,396 / 97,064 |
| q6-2 | 0 / 0 / 0 / 2.1 | yes, yes | 30,264 / 65,028 | 58,824 / 95,384 | 64,256 / 100,816 |
| main-3 | 0 / 0 / 0 / 2.24 | yes, yes | 30,308 / 60,056 | 58,848 / 91,736 | 65,068 / 97,944 |
| q6-3 | 0 / 0 / 0 / 2.18 | yes, yes | 30,244 / 64,344 | 58,928 / 95,072 | 64,604 / 100,748 |

KiB, GPU-driver mappings apart (`mem.rs`). Every run's RSS at its end is above its idle RSS (none paged out, none
skipped).

**Gate 3 (30 s idle with a pending prompt and a demoted worker = 0 frames): pass.** All six runs had 0 presents, 0 app
frames and 0 layer paints, with `prompt_pending: true` and `demoted_worker: true`.

**Gate 5 (memory within +5 % of main), means over three runs:**
- anon: after the 5 × 5 **+0.18 %**, at the end **+0.01 %**;
- USS: after the 5 × 5 **+4.92 %**, at the end **+3.76 %**.

Both are within +5 %, but USS after the 5 × 5 is at the edge. The USS gap was already there **at idle, before any
scene**: Q6 had 61,872–65,028 against main's 56,864–60,056 (mean +5,708 KiB), with anon equal (30,236–30,264 against
30,168–30,368). From idle to after the 5 × 5, USS grew 30,256 KiB on main and 30,456 on Q6 (means). So the gap is an
idle offset of private file-backed pages, not growth: the binary is larger (palette code, the JP face +644 B), and Q3's
round 3 saw the same kind of offset. The palette is never opened in `--bench-motion`.

## The palette left open (`idlepal.sh`, `idle-palette-frames-per-second.txt`)

These are the `--offline` demo's app frames per second (`SYNAPS_DESKTOP_LOG=debug`, `synaps_desktop::frame`), 45 s
each, measured on one run each:
- main at rest: 1/s (the demo's clocks);
- Q6 at rest: 1/s;
- **Q6 with `--palette goto` left open: 3/s.**

So an open palette costs 2 app frames a second. That is the rate of a caret blinking at 500 ms in the focused query
field, but this run does not isolate the cause. Closed, Q6 draws what main draws.
