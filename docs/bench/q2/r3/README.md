# Q2 r3: gates 3 and 5 again, 3 runs each, main built the same way as Q2

`run.sh` on bella runs `--bench-motion` three times, then `--bench` three times, alternating main and Q2. Each
run is in a headless gamescope at 144 Hz and 1920×1080, after the load average falls under 6 (or 180 s,
whichever comes first). Then an attribution pass: each app sits idle 12 s on the `--offline` demo, and its
`/proc/<pid>/smaps` is summed per mapped file (`*-idle-*.smaps.txt`, two runs each).

**Binaries** (`sums.txt`, with `size -A` of both):
- **main:** 4f87411, built by `dbuild synaps-desktop build --release --locked`. That is the same command,
  profile and toolchain as Q2's build. 38,109,544 bytes, sha256 `4e81d22b…`.
- **Q2:** a02a930, built by `dbuild synaps-desktop-q2 build --release --locked`. 38,184,432 bytes, sha256 `66a9ab19…`.
- Both are fresh copies in `~/.cache/q2-mem`, mapped by nothing else. Both are read whole into the page
  cache before every run.

The r1 bench (`../README.md`) was one run each, against the installed `synaps-desktop-hybrid.q4`.

## Gate 5: memory, medians of 3, GPU-driver mappings excluded (`mem.rs`), kB

| point | n | main RSS | Q2 RSS | Δ | main PSS | Q2 PSS | Δ | main anon | Q2 anon | Δ |
|---|---|---|---|---|---|---|---|---|---|---|
| `--bench-motion` idle | 3/3 | 97,460 | 96,388 | -1.1 % | 73,432 | 71,726 | -2.3 % | 30,164 | 30,144 | -0.1 % |
| `--bench-motion` end (after 5×5 load) | 3/3 | 126,748 | 126,068 | -0.5 % | 101,840 | 101,460 | -0.4 % | 58,112 | 57,488 | -1.1 % |
| `--bench` idle | 3/3 | 97,232 | 96,276 | -1.0 % | 72,550 | 71,644 | -1.2 % | 30,168 | 30,212 | +0.1 % |
| `--bench` long transcript | 3/3 | 152,240 | 151,664 | -0.4 % | 127,461 | 126,943 | -0.4 % | 83,276 | 83,272 | -0.0 % |
| `--bench` end | 3/3 | 162,408 | 162,180 | -0.1 % | 137,619 | 137,100 | -0.4 % | 93,868 | 93,608 | -0.3 % |

Every run is listed in `summary.txt`. The run-to-run spread of **one** binary is larger than r1's deltas:
- `--bench-motion` end PSS for main: 87,478 / 103,423 / 101,840.
- `--bench` idle PSS for main: 71,993 / 72,550 / 72,814.

r1 had idle PSS at +5.3 % (`--bench`) and +6.1 % (`--bench-motion`), one run each against the installed
`.q4`. Those deltas don't reproduce here. With three runs each, Q2 is at or below main at every point, and
anonymous memory is equal within 1.1 %.

## Where it would have come from: the binary

- **Size:** Q2's binary is 74,888 bytes larger than main's (+0.20 %). By section (`sums.txt`):
  - `.text` +67,968
  - `.eh_frame` +3,900
  - `.rodata` +1,024
  - `.rela.dyn` +888
  - `.data.rel.ro` +656
  - `.eh_frame_hdr` +568
  - `.got` +24
  - `.data` +8
  - `.relro_padding` −1,176
  - `.bss` 0
- **Statics:** `.rodata` grew 1 KB in all, so `authorize.rs` adds no large statics. The `.text` growth is the
  card's code: about 1,500 lines of gpui builder code.
- **Resident at idle:** the attribution pass found the binary's own resident pages *smaller* in Q2:
  30,748 / 30,620 kB against main's 31,772 / 31,684 kB.
- **Shared libraries:** their PSS swings between runs of the same binary, for example
  `libnvidia-gpucomp` at 6,033 vs 12,135 kB in main's two runs. PSS splits a shared page among every
  process that maps it, so it depends on what else is running.
- **Verdict:** nothing here is worth cutting.

## Gate 3: 30 s idle with a prompt pending (`--bench-motion` → `idle_30s_prompt`, `prompt_pending: true`)

main: 0 / 0 / 0 frames presented, 0 app frames. Q2: **0 / 0 / 0 frames presented, 0 app frames**. CPU:
main 2.83 / 1.55 / 2.74 ms/s, Q2 1.53 / 1.50 / 1.99 ms/s.
