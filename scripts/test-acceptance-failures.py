#!/usr/bin/env python3
"""Executable failure-path acceptance. Reports are allowlisted; raw logs are temporary."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CASES = ("mutations", "startup", "browser-timeout", "sigint", "sigterm", "docker-partial", "kind-partial")


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).splitlines()


def inventory():
    return dict(containers=set(docker("ps", "-aq", "--no-trunc")),
                networks=set(docker("network", "ls", "-q", "--no-trunc")),
                volumes=set(docker("volume", "ls", "-q")),
                images=set(docker("image", "ls", "--format", "{{.Repository}}:{{.Tag}} {{.ID}}")))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, action="append")
    options = parser.parse_args()
    results = {}
    for case in options.case or CASES:
        before = inventory()
        with tempfile.TemporaryDirectory(prefix="bootleg-failure-observer-") as scratch:
            scratch = Path(scratch)
            sentinel = scratch / "unrelated-credential"
            sentinel.write_bytes(os.urandom(32))
            original = sentinel.read_bytes()
            args = [sys.executable, str(ROOT / "scripts/test-device-auth.py")]
            if case == "mutations":
                args += ["--mutation-failures"]
            else:
                args += ["--fail-at", case]
                if case == "docker-partial":
                    args += ["--docker"]
                if case == "kind-partial":
                    args += ["--kind"]
            with (scratch / "raw.log").open("w+") as log:
                status = subprocess.run(args, cwd=ROOT, stdout=log, stderr=log).returncode
                log.seek(0)
                matches = re.findall(r"Sanitized scenario report: (.+)", log.read())
            assert len(matches) == 1, f"{case}: no unique sanitized report; not counted as passing"
            report_path = Path(matches[0])
            assert report_path.parent.parent == ROOT / ".pi/acceptance"
            assert re.fullmatch(r"bootleg-acceptance-[0-9a-f]{12}", report_path.parent.name)
            report = json.loads(report_path.read_text())
            prefix = report_path.parent.name
            assert report["cleanup"] == "completed" and report["private_files_removed"] and report["processes_removed"], case
            assert not any(prefix in line for line in docker("ps", "-a", "--format", "{{.Names}}")), case
            assert not any(prefix in line for line in docker("network", "ls", "--format", "{{.Name}}")), case
            assert not any(prefix in line for line in docker("image", "ls", "--format", "{{.Repository}}:{{.Tag}}")), case
            after = inventory()
            assert all(before[key] <= after[key] for key in before), f"{case}: unrelated resource disappeared"
            assert after["volumes"] == before["volumes"], f"{case}: unexpected retained volume"
            assert sentinel.read_bytes() == original
            if case == "mutations":
                assert status == 0 and report["failure_type"] is None
                names = ("ambiguous_creation", "ambiguous_rotation", "ambiguous_device_exchange")
                assert all(report["scenarios"][name].startswith("passed:") for name in names)
            else:
                expected = {"startup": "AssertionError", "browser-timeout": "TimeoutExpired",
                            "sigint": "KeyboardInterrupt", "sigterm": "KeyboardInterrupt",
                            "docker-partial": "InjectedAcceptanceFailure", "kind-partial": "RuntimeError"}
                assert status != 0 and report["failure_type"] == expected[case], case
                assert report["failure_injection"] == case and report["injection_reached"], case
            results[case] = dict(passed=True, owned_resources_removed=True,
                                 unrelated_resources_preserved=True, private_files_removed=True,
                                 processes_removed=True, source_report=prefix,
                                 revisions=report["revisions"], tracked_edits=report["tracked_edits"])
            print(f"PASS: {case}; cleanup independently verified", flush=True)
        destination = ROOT / ".pi/acceptance/failure-matrix.json"
        destination.parent.mkdir(parents=True, exist_ok=True)
        # Only fixed fields above are exported, never raw logs, DTOs or environment.
        with open(destination, "w", opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
            json.dump(results, output, indent=2)
            output.write("\n")


if __name__ == "__main__":
    main()
