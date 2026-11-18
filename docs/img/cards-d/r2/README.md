# Cards D round 2: evidence

The finish gate's must-fix list (`cohesion/finish-gate-pr23.md`), Shady's review, and H2 "Turning lattice"
(`indicators/proposal.md` §6). Every run: a headless gamescope on bella (1920×1080, 144 Hz,
`MESA_VK_WSI_DEBUG=sw`), an absolute `XDG_CONFIG_HOME` under `~/.cache`, and either the `--offline` demo or the
bench's own scenes (`--bench-motion`). No daemon and no sandbox ran in round 2.

Strips are every Nth frame of a lossless clip, each frame labelled with its number (`fN`, 60 fps). The files here
are 128-colour previews. The lossless clips (libx264rgb `qp=0`, gbrp, 60 fps) are off-repo, in
`~/Jawz/workspace/synaps-desktop/cardsd-evidence/r2/clips/`.

| Clip | Scene |
|---|---|
| `states-{full,reduced,off}.mkv` (26 s) | `SYNAPS_BENCH_STATES=1`: one run with a call in each state, an authorize card arriving, a slow daemon (Allow clicked at f398, resolved at about f450), the turn's end, a click that unfolds the run (f780) |
| `states-fold-full.mkv` (26 s) | the same, then a click that folds the new run (f1418) |
| `states-h1-deny.mkv` | the same with `SYNAPS_DESK_TURN=0` (H1, no turn on change), answered Deny |
| `life-{full,reduced,off}.mkv` (30 s) | `SYNAPS_BENCH_LIFE`: calls queued → running → done, workers spawning (rail, tabs, chips, composer), a failure, a prompt, the turn's end |
| `open-close-{full,reduced,off}.mkv` (9 s) | `--offline`: a call's diff well closes (f175), then opens (f302) |

## The finish gate, item by item

| # | What | Strip (frames) |
|---|---|---|
| 1 | A call's body closes and opens on `glide` | `1-call-closes-full-f175-203`, `1-call-opens-full-f302-330`: 23–24 changed frames each. Reduced and Off (`1-call-opens-{reduced,off}-f302-306`): 2 frames. |
| 2 | Streaming output grows a line at a time | `2-stream-line-full-f242-257`: each `test refresh_k ... ok` line's clip opens over about 10 frames, its text lifting from 70 %. Reduced (`-reduced-`): in place. |
| 3 | A new call grows in and rises, the card's edges whole | `3-call-arrives-full-f297-313`: the card's bottom edge and caps stay drawn every frame (the frame is one layer outside the rows' clips). Reduced: one frame. |
| 4 | Success: no ring blink; the quarter-turn, then the tick written on | `4-success-turn-tick-{full,reduced,off}-f654-700` (a 4× crop of the glyph). Full: the lattice turns, its dots go out by diagonal under the tick, lit `ok`, then `dim`. Reduced and Off: a cut. |
| 5 | Error: never blank; the cross written on | `5-error-cross-{full,reduced,off}-f538-578` (s3, followed down the column; `yN` is its row): the streaming chase turns, its dots go out under the cross writing on, never a blank frame. Its open body then glides to the failure's output (`7-error-resize-full-f538-568`, 23 frames, f543–565), never a one-frame jump. |
| 6 | A started call never reads "Queued" again | `h2-changes-q1-r2-k9-full-f448-500` row 1: q1 turns from Queued to Running at f456 and stays there. Test: `a_started_call_never_reads_queued_again` (Queued, started, Queued again: still running). |
| 7 | Allow folds the card into its row, the column still | `7-allow-fold-full-f450-500`: the card folds up into the `Run rm -rf target/tmp` row; the rows under it come up into its place. Nothing above it moves, during or after: the room it gave up waits at the list's end until rows that grow in use it, and goes with the turn's end (`track.py` on `states-full`: the rows under the card rise f455–479 into its room, 611 → 384 px; nothing above it moves; at the turn's end, f1348–1361, the room goes with the run's fold). |
| 8 | The approval card arrives on `glide` | `8-card-arrives-full-f150-192`: the card arrives over f151–173, 23 frames (height from +40, alpha from +170); f181–192 is the first stream line, a different change. Reduced (`-f150-160`): in place. |
| 9 | Pressed is visible and continuous against a slow daemon | `9-allow-pressed-slow-daemon-full-f392-464`: from the click (f398) the Allow button is down and the card says "answer sent, waiting for the daemon" until the resolve (about f450), then it folds. `9-deny-pressed-slow-daemon-h1-…` is the same for Deny (the H1 run). Off: the same states, no motion. |
| 10 | The run unfolds and folds row by row | `10-run-unfold-full-f780-824` is mislabelled: it opens on the turn-end fold (f782), and its unfold grows out of view, so only the first row's fade shows (f818–822). Round 3's `../r3-*` re-film the folds mid-screen. `10-run-fold-full-f1418-1432` (from `states-fold-full`, round 2): the fold's geometry was one frame (f1422), the other 8 the chevron turning. Round 3 (N1) makes it a spring: `../r3-fold-*`. Reduced (`10-run-unfold-reduced-f776-784`): a cut. |
| 11 | State words Sans 600; verbs never cut | `states-hold.png`, `states-folded.png`, `h2-ten-states-2x.png`: `Queued`, `Waiting for you`, `Exit 1`, `Denied` and the other state words are in Sans 600; `Activate` reads in full. |
| 12 | Evidence | Round 1's orbit and tick strips are removed (mis-cropped; the orbit is gone). Its `call-*` fold strips are renamed `run-*-4calls-*`. Cancel: `12-cancel-k9-{full,off}-f448-480` (k9 followed down the column: the streaming chase turns, its dots go out under the cancel mark, f456–466). Outline Allow: `outline-{rest,hover,pressed}.png` (filled: `fill-{hover,pressed}.png`). |

## H2 "Turning lattice"

- `h2-ten-states-2x.png`: the ten states at 2×: Queued, Running, Streaming, Seen, Done, Failed, Limit, Wait,
  Denied and Cancelled. These are rows from `states-hold.png` / `states-seen.png`.
- `h2-surfaces-{full,reduced,off}-f900-1020.png`: one lattice per surface (rail ×2, tab, a spawn row, a chip,
  the composer's state cell), every 4th frame of the life scene, at 3×. Full: running steps 4 times a second, all
  in phase. Reduced and Off: the still pose, no frames.
- `h2-changes-q1-r2-k9-full-f448-500.png`: three calls changing in one frame (f452), each followed down the column
  (`yN`): q1 Queued → Running, r2 Running → Done (the quarter-turn, then the tick writing on, lit `ok`), and k9
  Streaming → Cancelled (the turn, then the cancel mark).

## Allow, filled and outline

`fill-hover.png`, `fill-pressed.png` (the default); `outline-rest.png`, `outline-hover.png` and
`outline-pressed.png` (`SYNAPS_DESK_ALLOW=outline`). Outline's charge is `acc_line` on the card, its hover
`acc_soft`, and its press the accent's 18 % on the card.

## Reduced and Off

`open-close-*`: one or two changed frames for each open and close. In `states-*` and `life-*` (`runs.py`, the
transcript's box):

- **Off:** 21 and 15 runs of change, none longer than 2 frames. Nothing moves; every change is a cut.
- **Reduced:** 33 and 38 runs. The 7 longer ones are colour only (a wash's flush, the receipt's light, the
  composer's word), shortened to `quick` or less. Spatial changes are cuts. The lattice never steps: 0 frames
  between changes.

## Merge with main (#21)

From the merged head's release build:
- `merge-21-chips-f960.png` and `merge-21-chip-3x.png` show the life scene's worker chips at f960. Each is Q5's fixed-width chip, with H2's running lattice as its glyph, whole and on the chip's ground.
- `merge-21-rail-fold-f178-214.png` is the `--offline` demo's rail folding on `sheet` (`ctrl+b`), every 4th frame. Its running and needs-you rows slide under the window's edge with the content.

In `railfold-merged.mkv` (off-repo, `cardsd-evidence/r3/clips/`) the rail band changes on 11 single frames before the fold (lattice pose steps) and on none while folded (f217–360). The only changes then are the composer's state cell and its clock.
