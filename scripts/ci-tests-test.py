#!/usr/bin/env python3
"""Tests of scripts/ci-tests.py on made-up lanes: each way a lane can lose
or fail a test is an error, and a whole, passing lane is not.

usage: python3 scripts/ci-tests-test.py
"""

import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("ci_tests", os.path.join(os.path.dirname(__file__), "ci-tests.py"))
ci_tests = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci_tests)

DESKTOP = "synaps-desktop::bin/synaps-desktop"
FIXTURES = "synaps-client::fixtures"
MATCH = {"status": "matches"}


def case(match=MATCH, ignored=False):
    return {"kind": "test", "ignored": ignored, "filter-match": match}


def inventory():
    """A workspace lane's list: binary -> (status, {test: case})."""
    return {
        DESKTOP: ("listed", {
            "ui_tests::a": case(),
            "split_tests::the_split_on_lane_builds_every_view_split_on": case(),
            "model::b": case(),
            "split_tests::slow": case({"status": "mismatch", "reason": "ignored"}, ignored=True),
        }),
        FIXTURES: ("listed", {ci_tests.FIXTURE: case(), "other": case()}),
        "synaps-client::bin/synaps-probe": ("listed", {}),
    }


def ran(suites):
    """Every selected test, passed: [(binary, test, outcome)]."""
    return [(b, t, "passed") for b, (_, cases) in suites.items() for t, c in cases.items() if c["filter-match"] == MATCH]


class Lane(unittest.TestCase):
    def check(self, name="Linux-workspace", suites=None, slices=None, doc="test result: ok. 2 passed; 0 failed"):
        """ci-tests.py's errors for a lane: `suites` listed, `slices` the
        runs of each slice ([(binary, test, outcome)], None: no JUnit)."""
        suites = inventory() if suites is None else suites
        if slices is None:
            runs = ran(suites)
            slices = [runs[0::2], runs[1::2]]
        with tempfile.TemporaryDirectory() as root:
            d = os.path.join(root, f"list-{name}")
            os.makedirs(d)
            doc_json = {"rust-suites": {b: {"binary-id": b, "status": st, "testcases": cases} for b, (st, cases) in suites.items()}}
            with open(os.path.join(d, "tests.json"), "w") as f:
                json.dump(doc_json, f)
            with open(os.path.join(d, "slices"), "w") as f:
                f.write(f"{len(slices)}\n")
            if doc is not None:
                with open(os.path.join(d, "doctests.log"), "w") as f:
                    f.write(doc + "\n")
            for k, runs in enumerate(slices, 1):
                if runs is None:
                    continue
                s = os.path.join(root, f"junit-{name}-{k}")
                os.makedirs(s)
                with open(os.path.join(s, "junit.xml"), "w") as f:
                    f.write('<testsuites><testsuite name="x">')
                    for b, t, outcome in runs:
                        inner = {"passed": "", "failed": "<failure/>", "skipped": "<skipped/>"}[outcome]
                        f.write(f'<testcase classname="{b}" name="{t}" time="1.0">{inner}</testcase>')
                    f.write("</testsuite></testsuites>")
            with contextlib.redirect_stdout(io.StringIO()):
                return ci_tests.lane(root, name, [])

    def assertError(self, errors, text):
        self.assertTrue(any(text in e for e in errors), f"no error with {text!r} in {errors}")

    def test_a_whole_passing_lane_is_clean(self):
        self.assertEqual(self.check(), [])
        self.assertEqual(self.check("Linux-split-on", doc=None), [])

    def test_an_ignored_test_need_not_run(self):
        self.assertEqual(self.check(), [])  # split_tests::slow is #[ignore]d and absent from every slice

    def test_a_test_a_filter_left_out_of_the_list(self):
        suites = inventory()
        suites[DESKTOP][1]["model::b"] = case({"status": "mismatch", "reason": "default-filter"})
        self.assertError(self.check(suites=suites), "filtered out of the list (default-filter)")

    def test_an_ignored_test_filtered_for_another_reason(self):
        suites = inventory()
        suites[DESKTOP][1]["split_tests::slow"] = case({"status": "mismatch", "reason": "expression"}, ignored=True)
        self.assertError(self.check(suites=suites), "filtered out of the list (expression)")

    def test_a_binary_not_listed(self):
        suites = inventory()
        suites["synaps-client::events"] = ("skipped-default-filter", {})
        self.assertError(self.check(suites=suites), "binary synaps-client::events was not listed")

    def test_a_test_that_never_ran(self):
        runs = [r for r in ran(inventory()) if r[1] != "model::b"]
        self.assertError(self.check(slices=[runs]), f"listed but never ran: {DESKTOP} model::b")

    def test_a_test_skipped_in_its_slice_never_ran(self):
        runs = [(b, t, "skipped" if t == "model::b" else o) for b, t, o in ran(inventory())]
        self.assertError(self.check(slices=[runs]), f"listed but never ran: {DESKTOP} model::b")

    def test_a_test_run_twice(self):
        runs = ran(inventory())
        self.assertError(self.check(slices=[runs, runs[:1]]), "ran 2 times")

    def test_a_failed_test(self):
        runs = [(b, t, "failed" if t == "model::b" else o) for b, t, o in ran(inventory())]
        self.assertError(self.check(slices=[runs]), f"FAILED {DESKTOP} model::b (slice 1)")

    def test_a_slice_without_its_report(self):
        self.assertError(self.check(slices=[ran(inventory()), None]), "slice 2/2 wrote no JUnit file")

    def test_a_test_not_in_the_list(self):
        runs = ran(inventory()) + [(DESKTOP, "model::new", "passed")]
        self.assertError(self.check(slices=[runs]), f"ran but not listed: {DESKTOP} model::new")

    def test_no_ui_test(self):
        suites = inventory()
        del suites[DESKTOP][1]["ui_tests::a"]
        self.assertError(self.check(suites=suites), "no UI test ran")

    def test_the_fixture_outside_the_workspace_build(self):
        suites = inventory()
        del suites[FIXTURES]
        self.assertError(self.check(suites=suites), f"{ci_tests.FIXTURE} ran 0 times")

    def test_the_split_on_lane_without_its_test(self):
        suites = inventory()
        del suites[DESKTOP][1]["split_tests::the_split_on_lane_builds_every_view_split_on"]
        self.assertError(self.check("Linux-split-on", suites=suites, doc=None), f"{ci_tests.SPLIT_ON} did not run")

    def test_failed_or_missing_doc_tests(self):
        self.assertError(self.check(doc="test result: FAILED. 1 passed; 1 failed"), "the doc-tests did not pass")
        self.assertError(self.check(doc=None), "no doc-test log")

    def test_no_list(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertError(ci_tests.lane(root, "Linux-workspace", []), "no test list")


if __name__ == "__main__":
    unittest.main()
