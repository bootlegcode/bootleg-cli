#!/usr/bin/env python3
"""Run full acceptance privately; publish only normalized, allowlisted evidence."""
import argparse
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


def resources_removed(prefix):
    if not re.fullmatch(r"bootleg-acceptance-[0-9a-f]{12}", prefix):
        raise ValueError("invalid resource owner")
    for command in (["docker", "ps", "-a", "--format", "{{.Names}}"],
                    ["docker", "network", "ls", "--format", "{{.Name}}"],
                    ["docker", "image", "ls", "--format", "{{.Repository}}"]):
        names = subprocess.check_output(command, text=True).splitlines()
        if any(name == prefix or name.startswith(prefix + "-") for name in names):
            return False
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expect-failure-after-kind", action="store_true")
    options = parser.parse_args()
    destination = ROOT / ("ci-cleanup-report.json" if options.expect_failure_after_kind else "ci-report.json")
    destination.unlink(missing_ok=True)
    # Reject stale reports rather than accidentally publishing an earlier success.
    reports = ROOT / ".pi/acceptance"
    before = set(reports.glob("*/report.json"))
    for repo in (ROOT, *(ROOT.parent / name for name in json.loads(
            (ROOT / "scripts/acceptance-revisions.json").read_text()))):
        if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo):
            raise SystemExit("CI requires clean tracked files")
    # Raw stdout/stderr may contain credentials on failures. Never stream or upload it.
    with tempfile.TemporaryDirectory(prefix="bootleg-ci-private-") as scratch, tempfile.TemporaryFile() as log:
        command = ["python3", "scripts/test-device-auth.py", "--kind"]
        if options.expect_failure_after_kind:
            command.append("--fail-after-kind")
        child = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=log,
                                 env=dict(os.environ, TMPDIR=scratch))
        def forward(signum, _frame):
            if child.poll() is None:
                child.send_signal(signum)
        previous = {sig: signal.signal(sig, forward) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            returncode = child.wait()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)
    # Chromium and toolchains may leave auxiliary scratch files outside the
    # harness's private home. The wrapper owns this entire directory and removes
    # it on both success and failure; verify the completed deletion, not whether
    # third-party tools happened to leave it empty before teardown.
    private_files_removed = not Path(scratch).exists()
    created = set(reports.glob("*/report.json")) - before
    if len(created) != 1:
        raise SystemExit("Acceptance failed before producing one sanitized report")
    report_path = created.pop()
    report = json.loads(report_path.read_text())
    summary = summarize(report)
    if type(report.get("disk_free_bytes")) is int and report["disk_free_bytes"] >= 0:
        summary["disk_free_bytes_at_cleanup"] = report["disk_free_bytes"]
    if report.get("kind_failure") in ("command_timeout", "command_failed", "disk_exhausted", "image_pull_failed", "readiness_timeout"):
        summary["kind_failure"] = report["kind_failure"]
    if report.get("failure_type") in ("AssertionError", "CalledProcessError", "TimeoutExpired", "InjectedAcceptanceFailure"):
        summary["failure_type"] = report["failure_type"]
    if report.get("browser_failure_stage") in ("command", "chromium_start", "cdp_connect", "page_navigation", "initial_live"):
        summary["browser_failure_stage"] = report["browser_failure_stage"]
    summary["failure_locations"] = [
        dict(script=frame["script"], line=frame["line"])
        for frame in report.get("failure_locations", [])
        if frame.get("script") in {"test-device-auth.py", "acceptance_kind.py", "acceptance_network.py"}
        and type(frame.get("line")) is int and 0 < frame["line"] < 10000]
    docker_resources_removed = resources_removed(report_path.parent.name)
    summary["private_files_removed"] = private_files_removed
    summary["docker_resources_removed"] = docker_resources_removed
    cleanup_verified = (private_files_removed and docker_resources_removed
                        and summary["cleanup"] == "completed")
    summary["cleanup_verified"] = cleanup_verified
    summary["registration_complete"] &= cleanup_verified
    expected_failure = (options.expect_failure_after_kind and returncode != 0
                        and report["failure_type"] == "InjectedAcceptanceFailure"
                        and summary["scenarios"].get("kind") == "passed"
                        and not any(summary["tracked_edits"].values()) and cleanup_verified)
    if options.expect_failure_after_kind:
        summary["injected_failure_cleanup_passed"] = expected_failure
    destination.write_text(json.dumps(summary, indent=2) + "\n")
    os.chmod(destination, 0o600)
    if expected_failure:
        print("Injected post-kind failure cleaned owned resources and private files")
        return
    if returncode or not summary["registration_complete"] or options.expect_failure_after_kind:
        raise SystemExit("Full registration incomplete; consult sanitized report only")
    print("Full registration passed; cleanup completed")


if __name__ == "__main__":
    main()
