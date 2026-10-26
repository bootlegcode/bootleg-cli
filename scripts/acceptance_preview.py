"""Read-only preview acceptance using the actual web/stores and registered agents."""
import json
import os
from pathlib import Path
import subprocess
import time


def verify_preview(base, env, home, org, smuggler, children, log, outsider_env, postgres):
    repo = home / "preview-source"
    repo.mkdir()
    config = home / "preview-config.json"
    token_path = home / "preview-agent-token"

    def command(args, *, cwd=home, settings=env, status=0):
        p = subprocess.run(base + args, cwd=cwd, env=settings, capture_output=True, text=True, timeout=125)
        assert p.returncode == status, "preview CLI status mismatch: " + p.stdout + p.stderr
        return json.loads(p.stdout), p.stdout

    project, _ = command(["projects", "create", "Preview acceptance", "--org-id", org, "--token-file", str(token_path)])
    project_id = project["project_id"]
    command(["--config", str(config), "init", "--project-id", project_id, "--org-id", org])

    def git(*args):
        return subprocess.check_output(["/usr/bin/git", *args], cwd=repo, env=dict(env, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1"), text=True).strip()

    git("init", "-q")
    git("config", "user.email", "preview@example.invalid")
    git("config", "user.name", "Preview")
    (repo / "file").write_text("first\n")
    git("add", ".")
    git("commit", "-qm", "first")
    first = git("rev-parse", "HEAD")
    (repo / "file").write_text("second\n")
    git("add", ".")
    git("commit", "-qm", "second")
    second = git("rev-parse", "HEAD")
    agents = []
    for commit, dirty in [(first, "false"), (second, "false"), (None, None), (first, "true")]:
        settings = {k: env[k] for k in ("PATH", "HOME", "TERM")}
        settings.update(BOOTLEG_API_URL=base[base.index("--api-url") + 1], BOOTLEG_ALLOW_INSECURE="true",
                        BOOTLEG_PROJECT_ID=project_id, BOOTLEG_PROJECT_TOKEN=token_path.read_text().strip())
        if commit:
            settings.update(BOOTLEG_BUILD_COMMIT=commit, BOOTLEG_BUILD_DIRTY=dirty)
        proc = subprocess.Popen([str(smuggler), "--name", "preview acceptance", "--", "/bin/sleep", "600"],
                                cwd=home, env=settings, stdout=log, stderr=log)
        children.append(proc)
        agents.append(proc)

    def instances():
        result, _ = command(["instances", "list", "--project-id", project_id])
        return [row["container"] for row in result["items"]]

    end = time.monotonic() + 35
    rows = []
    while time.monotonic() < end:
        rows = instances()
        if len(rows) == 4 and all(r["presence"] == "connected" for r in rows):
            break
        time.sleep(.5)
    assert len(rows) == 4 and all(r["presence"] == "connected" for r in rows)
    clean = {r["build_commit"]: r["container_id"] for r in rows if r["build_dirty"] is False}
    unknown = next(r["container_id"] for r in rows if r["build_commit"] is None)
    dirty = next(r["container_id"] for r in rows if r["build_dirty"] is True)

    def preview(ids, status, settings=env):
        args = ["--config", str(config), "diff"]
        for cid in ids:
            args += ["--container-id", cid]
        result, text = command(args, cwd=repo, settings=settings, status=status)
        assert result["schema_version"] == 1 and result["filesystem_verified"] is False
        assert token_path.read_text().strip() not in text
        if status == 2:
            assert result["targets"] == [] and result["status"] == "error"
        return result, text

    # Exclude access-time changes; every file byte and entry must remain identical.
    def snapshot():
        return {str(p.relative_to(repo)): (None if p.is_dir() else p.read_bytes()) for p in repo.rglob("*")}

    before = snapshot()
    # Real-store retained/offline metadata fixtures force the registered selected
    # targets onto a later API page. No HTTP or WebSocket responses are mocked.
    assert project_id.startswith("project_") and project_id.replace("_", "").isalnum()
    sql = f"""INSERT INTO container_presence(project_id, container_id, record, retain_until)
        SELECT project_id, 'container_' || repeat('0',23) || lpad(to_hex(n),3,'0'),
            jsonb_set(record, '{{container_id}}', to_jsonb('container_' || repeat('0',23) || lpad(to_hex(n),3,'0'))),
            now() + interval '1 hour'
        FROM (SELECT project_id, record FROM container_presence WHERE project_id='{project_id}' LIMIT 1) seed,
            generate_series(1,201) n"""
    subprocess.run(["docker", "exec", postgres, "psql", "-U", "postgres", "-d", "cli_test", "-v", "ON_ERROR_STOP=1", "-c", sql],
                   check=True, stdout=log, stderr=log, timeout=30)
    assert len(instances()) == 205
    clean_report, _ = preview([clean[second]], 0)
    assert clean_report["targets"][0]["changes"] == []
    _, a = preview([clean[second], clean[first], clean[first]], 1)
    _, b = preview([clean[first], clean[second]], 1)
    assert a == b
    assert snapshot() == before
    (repo / "file").write_text("staged\n")
    git("add", "file")
    (repo / "file").write_text("second\n")
    before = snapshot()
    preview([clean[second]], 0)
    assert snapshot() == before
    for cid, code in [(unknown, "baseline_unknown"), (dirty, "baseline_dirty")]:
        report, _ = preview([clean[second], cid], 2)
        assert any(e["code"] == code for e in report["errors"])
    denied, _ = preview([clean[second]], 2, outsider_env)
    assert denied["errors"][0]["code"] == "discovery_failed"
    assert snapshot() == before
    (repo / ".env").write_text("never-publish-this-secret")
    report, text = preview([clean[second]], 2)
    assert report["errors"][0]["code"] == "sensitive_path"
    assert ".env" not in text and "never-publish" not in text
    (repo / ".env").unlink()
    for proc in agents:
        proc.terminate()
        proc.wait(timeout=15)
    end = time.monotonic() + 55
    while time.monotonic() < end:
        if all(r["presence"] == "offline" for r in instances()):
            break
        time.sleep(.5)
    report, _ = preview([clean[second]], 2)
    assert report["errors"][0]["code"] == "target_unavailable"
    return "passed: real registered agents; real-store fixtures force pagination; differing/unknown/dirty/offline baselines, denied access, deterministic exits 0/1/2 and read-only snapshots"
