# Q2 Authorize: gates 3 and 5, main vs Q2 (one run each)

**Superseded by [`r3/`](r3/README.md):** the +5.3 % / +6.1 % idle PSS below came from one run each against a
differently built binary (the installed `.q4`), inside PSS noise. With both binaries built the same way and
three runs each, there is no measurable difference (anon equal within 1.1 %).

`run.sh` on bella: `--bench-motion` then `--bench`, alternating main and q2, each in a headless gamescope at
144 Hz and 1920×1080, after the 1-minute load average falls under 6. This follows `docs/bench/q4/r4/run.sh`.
main is the installed `synaps-desktop-hybrid.q4` (609cf04's tree, identical to 4f87411); q2 is 4475cdf.

**Gate 3: 30 s idle with a prompt pending** (`--bench-motion` → `idle_30s_prompt`, after its 3 breaths,
with a demoted worker; `prompt_pending: true`): main 0 frames presented, 0 app frames; Q2 **0 frames presented, 0 app frames**.
CPU over those 30 s: main 1.84 ms/s, Q2 2.41 ms/s.

**Gate 5: memory** (`--bench`, from `mem.rs` over `/proc/self/smaps`, GPU-driver mappings excluded), kB:

| point | main RSS | Q2 RSS | Δ | main PSS | Q2 PSS | Δ |
|---|---|---|---|---|---|---|
| `mem_idle` | 94 160 | 97 572 | +3.6 % | 62 721 | 66 070 | **+5.3 %** |
| `mem_long_transcript_loaded` (2,000+ rows) | 150 408 | 153 120 | +1.8 % | 118 872 | 121 502 | +2.2 % |
| `mem_end` (after stream, scroll, spins, drags) | 161 100 | 163 364 | +1.4 % | 129 564 | 131 745 | +1.7 % |

Anonymous memory is the same at idle (30 036 kB each) and at the end (93 740 vs 93 580). So the
difference is file-backed: the larger binary's pages. Idle PSS is 5.3 % over main, above the +5 % bound.
The gate's own points (after the long transcript and at the end) are within it. These numbers come from one
run each, not a median.
