#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("ci_registration", Path(__file__).with_name("ci-registration.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class ReportTests(unittest.TestCase):
    def report(self):
        names = ("bootleg-cli", "bootleg-core", "bootleg-web", "bootleg-smuggler")
        return dict(scenarios={"kind": "passed: secret detail must not propagate"},
                    revisions={name: "a" * 40 for name in names},
                    tracked_edits={name: False for name in names},
                    registration_complete=True, cleanup="completed", failure_type=None)

    def test_complete_and_allowlisted(self):
        report = self.report()
        report["raw_log"] = "secret"
        summary = ci.summarize(report)
        self.assertTrue(summary["registration_complete"])
        self.assertNotIn("secret", str(summary))

    def test_partial_failure_cleanup_and_dirty_are_not_success(self):
        for field, value in (("registration_complete", False), ("cleanup", "failed"),
                             ("failure_type", "RuntimeError"),
                             ("scenarios", {"kind": "skipped: unavailable"})):
            report = self.report()
            report[field] = value
            self.assertFalse(ci.summarize(report)["registration_complete"])
        report = self.report()
        report["tracked_edits"]["bootleg-cli"] = True
        self.assertFalse(ci.summarize(report)["registration_complete"])

    def test_reject_invalid_revision(self):
        report = self.report()
        report["revisions"]["bootleg-web"] = "main"
        with self.assertRaises(ValueError):
            ci.summarize(report)


if __name__ == "__main__":
    unittest.main()
