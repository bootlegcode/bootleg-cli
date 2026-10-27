#!/usr/bin/env python3
"""Run full acceptance privately; publish only normalized, allowlisted evidence."""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def summarize(report):
    scenarios = report["scenarios"]
    revisions = report["revisions"]
    edits = report["tracked_edits"]
    names = {"bootleg-cli", "bootleg-core", "bootleg-web", "bootleg-smuggler"}
    if not isinstance(scenarios, dict) or not scenarios:
        raise ValueError("missing scenarios")
    if set(revisions) != names or set(edits) != names:
        raise ValueError("invalid repositories")
    if any(not re.fullmatch(r"[0-9a-f]{40}", sha) for sha in revisions.values()):
        raise ValueError("invalid revision")
    if any(type(value) is not bool for value in edits.values()):
        raise ValueError("invalid edit flags")
    normalized = {}
    for name, value in scenarios.items():
        if not re.fullmatch(r"[a-z_]{1,64}", name):
            raise ValueError("invalid scenario")
        normalized[name] = "passed" if value.startswith("passed:") else "skipped"
    complete = (report["registration_complete"] is True
                and report["cleanup"] == "completed"
                and report["failure_type"] is None
                and not any(edits.values())
                and all(value == "passed" for value in normalized.values()))
    return dict(registration_complete=complete, scenarios=normalized,
                cleanup="completed" if report["cleanup"] == "completed" else "failed",
                revisions=revisions, tracked_edits=edits)


def main():
    destination = ROOT / "ci-report.json"
    destination.unlink(missing_ok=True)
    # Reject stale reports rather than accidentally publishing an earlier success.
    reports = ROOT / ".pi/acceptance"
    before = set(reports.glob("*/report.json"))
    for repo in (ROOT, *(ROOT.parent / name for name in json.loads(
            (ROOT / "scripts/acceptance-revisions.json").read_text()))):
        if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo):
            raise SystemExit("CI requires clean tracked files")
    # Raw stdout/stderr may contain credentials on failures. Never stream or upload it.
    with tempfile.TemporaryFile() as log:
        child = subprocess.Popen(["python3", "scripts/test-device-auth.py", "--kind"],
                                 cwd=ROOT, stdout=log, stderr=log)
        def forward(signum, _frame):
            if child.poll() is None:
                child.send_signal(signum)
        previous = {sig: signal.signal(sig, forward) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            returncode = child.wait()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)
    created = set(reports.glob("*/report.json")) - before
    if len(created) != 1:
        raise SystemExit("Acceptance failed before producing one sanitized report")
    summary = summarize(json.loads(created.pop().read_text()))
    destination.write_text(json.dumps(summary, indent=2) + "\n")
    os.chmod(destination, 0o600)
    if returncode or not summary["registration_complete"]:
        raise SystemExit("Full registration incomplete; consult sanitized report only")
    print("Full registration passed; cleanup completed")


if __name__ == "__main__":
    main()
