#!/usr/bin/env python3
"""CI's test report (.github/workflows/ci.yml, the `ci` job).

Each lane's tests are built once into an archive, listed from that
archive with no filter and no partition (`cargo nextest list
--archive-file ... --ignore-default-filter`, tests.json), and run in
slices on other runners, each slice writing a JUnit file. This checks,
per lane, what the single `cargo test` step used to check, across the
slices:

  * the list holds every test in the archive: each binary listed and
    each test either selected or `#[ignore]`d (a test or binary a filter
    left out is an error, as `cargo test` would have run it);
  * every test the list selected ran exactly once and passed (none lost
    between slices, none run twice), and nothing failed;
  * tests ran (passed > 0) and UI tests ran (`ui_tests::`);
  * workspace lanes: the client's fixture
    `catalog_page_requests_encode_as_the_daemon_expects` ran in the
    workspace build (#28: serde_json's `preserve_order`, unified there),
    and the doc-tests (which nextest does not run) passed;
  * the split-on lane: `split_tests::the_split_on_lane_builds_every_view_split_on`
    ran.

usage: ci-tests.py <artifacts-dir> <lane>...
  lane: <runner.os>-<name>, e.g. Linux-workspace; its files are
  <dir>/list-<lane>/tests.json, <dir>/list-<lane>/slices (how many),
  <dir>/list-<lane>/doctests.log (workspace lanes) and
  <dir>/junit-<lane>-<k>/**/junit.xml for each slice k.
"""

import collections
import json
import os
import re
import sys
import xml.etree.ElementTree as ET

FIXTURE = "catalog_page_requests_encode_as_the_daemon_expects"
SPLIT_ON = "split_tests::the_split_on_lane_builds_every_view_split_on"


def listed(path):
    """((binary, test) -> ignored, errors) from `cargo nextest list`'s JSON.

    A test is to run (filter-match `matches`) or `#[ignore]`d (`ignored`,
    not matched for that reason only). A binary not listed or a test not
    matched for any other reason (a default filter, an expression, a
    partition) was filtered out of a list taken with no filter: an error,
    never counted as ignored."""
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    tests, errors = {}, []
    for binary, suite in doc["rust-suites"].items():
        if suite.get("status") != "listed":
            errors.append(f"binary {binary} was not listed ({suite.get('status')}): its tests would never run")
        for name, case in suite["testcases"].items():
            match = case["filter-match"]
            if match["status"] == "matches":
                tests[(binary, name)] = False
            elif case.get("ignored") and match.get("reason") == "ignored":
                tests[(binary, name)] = True
            else:
                errors.append(f"filtered out of the list ({match.get('reason')}): {binary} {name}")
    return tests, errors


def junit(path):
    """[(binary, test, outcome, seconds)] from one slice's JUnit file."""
    out = []
    for case in ET.parse(path).getroot().iter("testcase"):
        tags = {child.tag for child in case}
        if tags & {"failure", "error", "rerunFailure", "flakyFailure"}:
            outcome = "failed"
        elif "skipped" in tags:
            outcome = "skipped"
        else:
            outcome = "passed"
        out.append((case.get("classname"), case.get("name"), outcome, float(case.get("time") or 0)))
    return out


def find(root, name):
    for d, _, files in os.walk(root):
        if name in files:
            return os.path.join(d, name)
    return None


def doctests(path):
    """(passed, ok) from `cargo test --doc`'s output."""
    with open(path, encoding="utf-8", errors="replace") as f:
        text = f.read()
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    results = re.findall(r"^test result: (\w+)\. (\d+) passed; (\d+) failed", text, re.M)
    return sum(int(p) for _, p, _ in results), bool(results) and all(r == "ok" and f == "0" for r, _, f in results)


def lane(root, name, summary):
    errors = []
    tests_json = os.path.join(root, f"list-{name}", "tests.json")
    if not os.path.exists(tests_json):
        return [f"{name}: no test list (the build job did not finish)"]
    tests, list_errors = listed(tests_json)
    errors += [f"{name}: {e}" for e in list_errors]
    with open(os.path.join(root, f"list-{name}", "slices"), encoding="utf-8") as f:
        slices = int(f.read())
    expected = {k for k, ignored in tests.items() if not ignored}
    runs = []
    for k in range(1, slices + 1):
        path = find(os.path.join(root, f"junit-{name}-{k}"), "junit.xml")
        if path is None:
            errors.append(f"{name}: slice {k}/{slices} wrote no JUnit file")
            continue
        runs += [(k, *r) for r in junit(path)]

    seen = collections.Counter((b, t) for _, b, t, outcome, _ in runs if outcome != "skipped")
    passed = {(b, t) for _, b, t, outcome, _ in runs if outcome == "passed"}
    failed = sorted(f"{b} {t} (slice {k})" for k, b, t, outcome, _ in runs if outcome == "failed")
    errors += [f"{name}: FAILED {f}" for f in failed]
    errors += [f"{name}: ran {n} times: {b} {t}" for (b, t), n in sorted(seen.items()) if n > 1]
    missing = sorted(expected - set(seen))
    errors += [f"{name}: listed but never ran: {b} {t}" for b, t in missing[:20]]
    if len(missing) > 20:
        errors.append(f"{name}: ... and {len(missing) - 20} more never ran")
    extra = sorted(set(seen) - expected)
    errors += [f"{name}: ran but not listed: {b} {t}" for b, t in extra[:20]]

    by_binary = collections.Counter(b for b, _ in passed)
    ignored = collections.Counter(b for (b, _), ig in tests.items() if ig)
    ui = sum(1 for _, t in passed if t.startswith("ui_tests::"))
    total = len(passed)
    doc = None
    if name.endswith("-workspace"):
        log = os.path.join(root, f"list-{name}", "doctests.log")
        if not os.path.exists(log):
            errors.append(f"{name}: no doc-test log")
        else:
            doc, ok = doctests(log)
            if not ok:
                errors.append(f"{name}: the doc-tests did not pass")
            total += doc
        fixtures = sum(1 for b, t in passed if t == FIXTURE)
        if fixtures != 1:
            errors.append(f"{name}: the client's fixture {FIXTURE} ran {fixtures} times in the workspace build, not once")
    if name.endswith("-split-on") and not any(t == SPLIT_ON for _, t in passed):
        errors.append(f"{name}: {SPLIT_ON} did not run")
    if total == 0:
        errors.append(f"{name}: no test passed")
    if ui == 0:
        errors.append(f"{name}: no UI test ran")

    per_slice = collections.defaultdict(float)
    for k, _, _, _, secs in runs:
        per_slice[k] += secs
    slowest = sorted(((secs, t) for _, _, t, outcome, secs in runs if outcome != "skipped"), reverse=True)[:5]

    summary.append(f"### {name}: {total} tests passed, {ui} of them UI tests, in {slices} slices")
    summary.append("")
    summary.append("| binary | passed | ignored |")
    summary.append("|---|---:|---:|")
    for b in sorted(set(by_binary) | set(ignored)):
        summary.append(f"| `{b}` | {by_binary[b]} | {ignored[b]} |")
    if doc is not None:
        summary.append(f"| doc-tests (`cargo test --doc`) | {doc} | 0 |")
    summary.append("")
    summary.append("Test time per slice (sum of its tests): " + ", ".join(f"{k}: {per_slice[k]:.0f} s" for k in sorted(per_slice)))
    summary.append("")
    summary.append("Slowest: " + "; ".join(f"`{t}` {s:.0f} s" for s, t in slowest))
    summary.append("")
    print(f"{name}: {total} tests passed ({ui} UI tests), {len(expected)} listed to run, {sum(ignored.values())} ignored; "
          + ", ".join(f"{b}: {n}" for b, n in sorted(by_binary.items()))
          + (f", doc-tests: {doc}" if doc is not None else ""))
    return errors


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    root, lanes = sys.argv[1], sys.argv[2:]
    errors, summary = [], []
    for name in lanes:
        errors += lane(root, name, summary)
    out = os.environ.get("GITHUB_STEP_SUMMARY")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write("\n".join(summary) + "\n")
    for e in errors:
        print(f"::error::{e}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
