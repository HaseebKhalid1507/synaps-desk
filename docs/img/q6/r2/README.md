# Q6 r2: evidence

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/q6/r2), at the same paths. See [EVIDENCE.md](../../../EVIDENCE.md).

**Binary:** head `781122b`, which is r2 merged with main `2bbe974` (Q7). Release build, sha256 `c5b9469d…468965bdf8`,
installed as `bella:~/.local/bin/synaps-desktop-hybrid.q6`. r1 is `ebf37ed` (`20f3017f…`).

**Config safety:** every app run's settings folder was a scratch folder under `~/.cache/q6r2` (`capture-linux.sh`'s
absolute `$out/config`). The real `~/.config/synaps-desktop/settings.json` kept its mtime (00:54:01) through every run.

## Live: the command path on the desktopproto sandbox (`live.sh`)

Run on bella with `synaps 0.11.1`, `synaps daemon --profile desktopproto`, started by this run and stopped after it
(`live.log` records it).

**Temporary changes to the sandbox config:**
- for the run, `~/.synaps-cli/desktopproto/config` got the two `auth.` lines (for the model calls) and
  `client_input = exclusive` (so another client can own the input);
- afterwards it was restored from its copy: sha256 `c1b7ac41…320edd`, the same before and after, with 0 `auth.` lines.

**Session set-up:** `synaps attach --create` made the session, and it asked one question (haiku-4-5, 2 messages).

The app ran at 1280×720 in nord. Each still is the window's pixmap (`import -window`, lossless), taken at the time
given in `live-<run>-drive.log`.

### Driving (`live-drive-*`)

The composer's `/budget`, `/context`, `/workers`, `/trace` and `/confirms`, then the palette's `/compact`.

| still | shows |
|---|---|
| `live-budget.png` | `/budget`'s whole answer, 4 lines (they wrap), in a well at the transcript's end. Note: `/budget: 4 lines ↓` |
| `live-context.png` | `/context`, 9 lines, folded at 8 (`… 1 more lines`, `9 lines · show all`) |
| `live-workers.png`, `live-trace.png`, `live-confirms.png` | `No workers tracked in this session.` (1 line), the trace status (3 lines), the confirms line |
| `live-compact-palette.png` | Ctrl+K `compact`: `/compact` selected. The `owner` trail is gone while driving (O1) |
| `live-compact-sent.png` → `live-compact-asked.png` | `/compact: sent, waiting for the daemon`, then the daemon's `{"kind":"none"}`, read as `/compact: asked (the log shows how it goes)` (H1) |
| `live-compact-after.png` | the daemon's `conversation compacted (2 messages)` under the five answers |

The answers' rows stayed through the compaction's history refold, and none was sent.

### Observing (`live-observe-*`)

A second client took the input: `synaps attach <id> --takeover`, with `exclusive` in force.

| still | shows |
|---|---|
| `live-observe-palette.png` | Ctrl+K `compact`: the engine rows dimmed and marked `owner` |
| `live-observe-refused.png` | Enter: `/compact is the session owner's: read-only here: another client owns this session's input`, and nothing sent |
| `live-observe-composer.png` | the composer's `/budget`: the same refusal, and the text stays in the composer |

The observing app is a new process, so the driving run's answer rows are not in it: they are client-side and not
persisted.

## Offline: the design gate's items (`design.sh`, `designs.sh`)

The `--offline` demo, the palette opened at launch (`--palette goto`), then keys: `led`, `f`, Tab, `f`, `fold`, `led`.

**Placement (fix 1):** `side-r1-r2-goto-<theme>-<size>.png` (r1 left, r2 right).

| | r1 box top / bottom | r2 box top / bottom | tab strip | composer frame |
|---|---|---|---|---|
| 1280×720 (nord, paper) | 72 / 586 | **96 / 575** | 51–85 | 591: r2 leaves a 15 px gap (r1: 4) |
| 1920×1080 (nord, paper) | 72 / 586 | **96 / 610** | 51–85 | below y 900: the list is at its 416 px cap |

- The top is the strip's laid-out bottom + 10.
- The list's height is bounded so that the box ends at least 16 px above the composer's laid-out frame.
- The measurement: the box frame's run of rows at its left edge, scanned in the stills.
- No tab label is cut. See `crop-placement-paper-1280-1.5x.png`.

**Named handles in mono (fix 2):** `crop-mono-handles-rail-tab-palette-nord-1920.png`. `relay-refresh`,
`ingest-core`, `ledger-drift`, `docs-sync` and `subconscious` are Plex Mono in the rail, the tab and the palette alike.
Free titles (`Flaky e2e: …`, `New session`) stay sans.

**Overflow (fix 3):**
- Each head counts its rows: `S E S S I O N S セッション 13`.
- The house scrollbar, on a new `thumb_k` role on the card, shows at rest when the list overflows. See `r2-*-goto-open`
  and `r2-*-cmds-open`.
- When nothing overflows, there is no bar. See `r2-nord-1280-cmds-fold.png`.

**Matches lit (fix 4):** `r2-nord-1280-goto-led.png` (`led` in `ledger-drift`), `r2-nord-1280-goto-f.png`,
`r2-nord-1280-cmds-f.png` and `crop-cmds-f-meta-match-nord-1280-2x.png`.
- The matched characters are drawn in `you`, with no fill.
- `Motion level` matched on its meta (`now full`), so the `f` is lit there.
- `hits_are_the_chars_ranked_on` checks that `hit` returns the characters it ranked on: char indices, multibyte too.

**VIEWS has 表示 (O4):** the JP subset was re-run. Only the JP face is committed (7,444 → 7,756 B), and the font
test covers the new characters.
