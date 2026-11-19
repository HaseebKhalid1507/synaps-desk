# Evidence media

Screenshots, frame sheets and screen recordings live in the private repo
[synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence), not here. They were moved out of this repo's history on 2026-11-19 so a
clone stays small.

- **Same paths.** A file named in a README under `docs/img/<dir>/` is at `docs/img/<dir>/<file>` in
  synaps-desk-evidence. Its READMEs and logs are copied there too.
- **Older media** (other branches, earlier review rounds, re-shot files) is under `history/` there; `history/INDEX.tsv`
  maps each file to its original path, blob id and pre-scrub commits.
- **New evidence goes there**, one folder per pull request: `pr-<n>/` with a `README.md`. Link to it from the PR as
  `https://github.com/HaseebKhalid1507/synaps-desk-evidence/blob/main/pr-<n>/<file>`. Don't commit screenshots or clips here.
- Still in this repo: `docs/img/dropout/before-frames-40-41.png` and `docs/img/dropout/ink-before-after.png`, the two
  images linked from `docs/bench/text-dropout/README.md`.

Commit SHAs quoted in the evidence READMEs are pre-scrub SHAs. They still open on GitHub (pull requests keep
them) but are not in a fresh clone.
