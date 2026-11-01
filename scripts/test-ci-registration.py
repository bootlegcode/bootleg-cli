#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("ci_registration", Path(__file__).with_name("ci-registration.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)

relay_spec = importlib.util.spec_from_file_location("acceptance_relay", Path(__file__).with_name("acceptance_relay.py"))
relay = importlib.util.module_from_spec(relay_spec)
relay_spec.loader.exec_module(relay)


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


class RelayBindingTests(unittest.IsolatedAsyncioTestCase):
    async def test_reject_wildcard_loopback_public_and_wrong_subnet(self):
        for address, subnet in (("0.0.0.0", "0.0.0.0/8"),
                                ("127.0.0.1", "127.0.0.0/8"),
                                ("8.8.8.8", "8.8.8.0/24"),
                                ("172.19.0.1", "172.18.0.0/16")):
            with self.subTest(address=address), self.assertRaises(ValueError):
                await relay.serve(address, subnet, 1)

    def test_resource_audit_rejects_unowned_prefix(self):
        for prefix in ("bootleg", "", "bootleg-acceptance-.*"):
            with self.assertRaises(ValueError):
                ci.resources_removed(prefix)


if __name__ == "__main__":
    unittest.main()
