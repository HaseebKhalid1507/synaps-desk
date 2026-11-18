# Q6 r2: binary size and USS by mapping (Shady #20 L7)

**Binaries** (release, `dbuild … build --release --locked -p synaps-desktop`, bella):

| build | commit | bytes | sha256 |
|---|---|---|---|
| main | `0a78bb7` | 38,325,432 | `ccea70a2…48bbe` |
| Q6 r1 | `ebf37ed` | 38,437,992 | `20f3017f…551080b2` |
| Q6 r2 (measured below) | `4f48d24` | 38,497,896 | `f53156ff…745cb6e0` |
| Q6 r2 (head, installed) | `761d406` | 38,497,512 | `597508fc…69e9112a` |

r2 is 172 KB (+0.45 %) larger than main.

**USS by mapping at idle** (`smaps.sh`): the `--offline` demo at 1920×1080 in a headless gamescope (144 Hz), its
config a scratch folder under `~/.cache/q6r2`. Sampled from `/proc/<pid>/smaps` 12 s after start, 3 runs each,
alternating main and r2, each run waiting for a load under 6. USS is `Private_Clean + Private_Dirty` (KiB).
The mappings are grouped as follows:
- **binary:** the executable's own file;
- **gpu driver:** `/dev/dri` and the Vulkan/Mesa/LLVM libraries;
- **other files:** every other file-backed mapping (libc, fonts and the rest);
- **anon:** everything without a file (heap, stacks, `[anon]`).

| mapping | main USS (3 runs) | Q6 r2 USS (3 runs) | mean Δ |
|---|---|---|---|
| binary | 32,780 / 33,528 / 33,020 | 33,328 / 33,416 / 33,252 | +223 KiB (+0.7 %) |
| anon (heap, stacks, [..]) | 40,004 / 38,360 / 37,320 | 38,192 / 37,408 / 37,436 | -883 KiB (-2.3 %) |
| gpu driver | 24,488 / 24,412 / 24,444 | 24,432 / 24,448 / 24,416 | -16 KiB (-0.1 %) |
| other files (libs, fonts) | 4,216 / 4,232 / 4,208 | 4,112 / 4,068 / 4,252 | -75 KiB (-1.8 %) |
| total | 101,488 / 100,532 / 98,992 | 100,064 / 99,340 / 99,356 | -751 KiB (-0.7 %) |

**What it shows:**
- At idle, the binary's resident private pages are the same within noise on both builds (+223 KiB mean, +0.7 %), and
  so is the total (−751 KiB, −0.7 %).
- The +4.5 MB idle offset in r1's `--bench-motion` runs (`../README.md`, USS 61,872 vs 57,180 at `mem_idle`) does not
  show here. That is a different scene: `--bench-motion` builds its synthetic transcript before `mem_idle`, and this
  is the demo at rest. So the offset is not the binary's size.
- I did not re-run `--bench-motion` for r2.

Files: `main-<n>.txt` and `r2-<n>.txt` (one run each), `smaps.sh`.
