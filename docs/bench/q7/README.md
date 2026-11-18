# Q7 Sticky: gates 3 and 5 against main `0a78bb7`

These ran on bella with `run.sh` (here). Main `0a78bb7` (sha256 `2bf0106a…`) and Q7 `3e2093d` (sha256 `5f88fb61…`) are
both release builds made through `dbuild` from the same tree. Each binary ran 3 times, alternating:
`--bench-motion`, then `--bench`, at 1920×1080, 144 Hz, headless gamescope, nord.
- Every run logs the load at its start and the system-wide swap-ins during it (`/proc/vmstat` `pswpin`), in `raw/log`.
- `summary.txt` is `sum.py` over `raw/`.

## Gate 3: idle = 0 presents (`idle_30s_prompt`: a prompt pending after its breaths, a demoted worker, 30 s)

| run | presents | app frames | layer paints | CPU ms/s | swap-ins during the run |
|---|---|---|---|---|---|
| main 1 | 0 | 0 | 0 | 1.94 | 7,196 |
| Q7 1 | 0 | 0 | 0 | 2.17 | 3,869 |
| main 2 | 0 | 0 | 0 | 1.97 | 5,787 |
| Q7 2 | 0 | 0 | 0 | 2.28 | 3,038 |
| main 3 | 0 | 0 | 0 | 2.29 | 37,138 |
| Q7 3 | 0 | 0 | 0 | 2.24 | 724 |

**Pass:** 0 presents, 0 app frames and 0 layer paints in all six runs. `prompt_pending` and `demoted_worker` are true in
all six.
- **Swap.** Every `--bench-motion` run (about 4.5 min each) saw system-wide swap-ins; bella swaps. Paging can't add a
  present, so the 0s stand. The CPU column is not a comparison. The counter is system-wide, so it doesn't say whether
  this app's pages were the ones paged in.

## Gate 5: memory after 2,000 rows of history (`--bench` `mem_end`, GPU-driver mappings apart, KiB)

| run | anon | USS | swap-ins |
|---|---|---|---|
| main 1 | 93,488 | 126,316 | 51 |
| Q7 1 | 93,456 | 124,360 | 340 |
| main 2 | 93,720 | 123,244 | 85 |
| Q7 2 | 93,772 | 124,316 | 45 |
| main 3 | 93,792 | 123,548 | 35 |
| Q7 3 | 94,608 | 125,464 | 37 |

**Means:** anon 93,667 → 93,945 (**+0.30 %**), USS 124,369 → 124,713 (**+0.28 %**). Both are within +5 %: **pass**. No memory
run paged much: 35–340 swap-ins each.
