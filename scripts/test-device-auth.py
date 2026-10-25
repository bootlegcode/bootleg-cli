#!/usr/bin/env python3
"""Real CLI/web acceptance test; disposable Docker stores, no API mocks.
Requires Python 3, Docker, fzf, Node, and sibling web Playwright/Chromium.
Account fixtures use HTTP forms; device approval uses actual Chromium UI.
"""
import argparse
import base64
import signal
import uuid
import fcntl
import struct
import sys
import termios
import http.cookiejar
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT.parent / "bootleg-web"
SMUGGLER = ROOT.parent / "bootleg-smuggler"
BIN = ROOT / "target/debug/bootleg"


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker", action="store_true", help="exercise Docker Desktop agent network lifecycle (macOS)")
    options = parser.parse_args()
    if options.docker and os.uname().sysname != "Darwin":
        parser.error("--docker currently requires macOS Docker Desktop host.docker.internal routing")
    assert shutil.which("fzf"), "install fzf"
    run("cargo", "build", "--locked", cwd=ROOT)
    run("cargo", "build", "--locked", "-p", "bootleg-web", cwd=WEB)
    run("cargo", "build", "--locked", cwd=SMUGGLER)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    resource_prefix = "bootleg-acceptance-" + uuid.uuid4().hex[:12]
    artifact_dir = ROOT / ".pi" / "acceptance" / resource_prefix
    artifact_dir.mkdir(parents=True, mode=0o700)
    results = {name: "skipped: not reached" for name in (
        "chromium_login", "interactive_config", "native_discovery", "browser_sse",
        "control_plane_outage", "restart_identity", "docker_network_lifecycle",
        "rotation", "deactivation", "browser_permission_loss", "multi_org_project_config",
        "expiry_logout_denial")}
    results.update({name: "skipped: not implemented" for name in (
        "kind", "cross_org_isolation", "stale_session_callbacks", "disabled_network", "hidden_tabs")})
    containers = []
    children = []
    docker_image = docker_network = None
    with tempfile.TemporaryDirectory(prefix="bootleg-cli-e2e-") as directory:
        home = Path(directory)
        env = {k: v for k, v in os.environ.items() if not k.startswith(
            ("BOOTLEG_", "PG", "DATABASE_URL", "VALKEY_", "OTEL_"))}
        env.update(HOME=directory, TERM="xterm-256color")
        try:
            def container(image, port, *args):
                cid = run("docker", "run", "--name", f"{resource_prefix}-{len(containers)}", "--rm", "-d", "-p", f"127.0.0.1::{port}", *args, image)
                containers.append(cid)
                address = run("docker", "port", cid, str(port)).splitlines()[0]
                return cid, address.rsplit(":", 1)[1]

            pg, pgport = container("postgres:17-alpine", 5432,
                                   "-e", "POSTGRES_PASSWORD=isolated-test", "-e", "POSTGRES_DB=cli_test")
            _, redisport = container("valkey/valkey:8-alpine", 6379)
            _, mailport = container("axllent/mailpit:latest", 1025)
            for _ in range(100):
                if subprocess.run(["docker", "exec", pg, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                    break
                time.sleep(.2)
            origin = f"http://127.0.0.1:{free_port()}"
            env.update(PGHOST="127.0.0.1", PGPORT=pgport, PGUSER="postgres",
                       PGPASSWORD="isolated-test", PGDATABASE="cli_test", PGSSLMODE="disable",
                       VALKEY_URL=f"redis://127.0.0.1:{redisport}/0",
                       BIND_ADDR=origin.removeprefix("http://"), BOOTLEG_PUBLIC_BASE_URL=origin,
                       BOOTLEG_INSECURE_COOKIES="true", BOOTLEG_EMAIL_LOCAL_MODE="true",
                       BOOTLEG_EMAIL_ALLOW_LOCAL_HTTP="true", BOOTLEG_EMAIL_PROVIDER="smtp",
                       BOOTLEG_EMAIL_SMTP_SECURITY="local_plaintext", BOOTLEG_EMAIL_SMTP_HOST="127.0.0.1",
                       BOOTLEG_EMAIL_SMTP_PORT=mailport, BOOTLEG_EMAIL_FROM_ADDRESS="noreply@example.test",
                       BOOTLEG_EMAIL_VERIFICATION_SIGNING_KEY=base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip("="))
            run(str(WEB / "target/debug/bootleg-web"), "migrate", cwd=WEB, env=env)
            log = open(home / "web.log", "w+")
            app = subprocess.Popen([str(WEB / "target/debug/bootleg-web"), "serve"],
                                   cwd=WEB, env=env, stdout=log, stderr=log)
            children.append(app)
            for _ in range(100):
                try:
                    urllib.request.urlopen(origin + "/health/ready", timeout=1).close()
                    break
                except OSError:
                    if app.poll() is not None:
                        log.seek(0)
                        raise AssertionError("web startup failed: " + log.read())
                    time.sleep(.2)
            cookies = http.cookiejar.CookieJar()
            browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))

            def browser_decision(path, name, decision="approve"):
                subprocess.run(["node", str(ROOT / "scripts/approve-device.cjs")],
                               input=json.dumps(dict(origin=origin, path=path, name=name,
                                   decision=decision, cookies=[dict(name=c.name, value=c.value)
                                                             for c in cookies])),
                               text=True, check=True, timeout=40)

            def page(path, fields=None):
                data = None if fields is None else urllib.parse.urlencode(fields).encode()
                with browser.open(origin + path, data=data, timeout=30) as response:
                    return response.read().decode()

            def csrf(html):
                return re.search(r'name="csrf" value="([^"]+)"', html)[1]

            html = page("/register")
            page("/register", dict(csrf=csrf(html), email="cli-test@example.test", password="test-only long password", timezone="UTC"))
            # Only this disposable account: eligibility fixture, not an auth bypass in the app.
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                "UPDATE users SET verified=true, email_verified_at=now(), email_verification_issued_at=NULL, email_verification_expires_at=NULL WHERE email='cli-test@example.test'")

            # Config still tests automatic opening without opening the developer's browser.
            tools = home / "bin"
            tools.mkdir()
            for opener in ("open", "xdg-open"):
                path = tools / opener
                path.write_text("#!/bin/sh\nexit 1\n")
                path.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + env["PATH"]
            base = [str(BIN), "--api-url", origin, "--allow-http", "--notices=never", "--no-color", "--json"]

            def cli(*args, ok=True):
                result = subprocess.run(base + list(args), cwd=home, env=env, capture_output=True, text=True, timeout=35)
                assert (result.returncode == 0) == ok, result.stderr + result.stdout
                return json.loads(result.stdout)

            def start_login(*args):
                proc = subprocess.Popen(base + list(args), cwd=home, env=env,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                children.append(proc)
                deadline = time.monotonic() + 30
                line = ""
                while "Approve Bootleg CLI" not in line:
                    assert time.monotonic() < deadline and proc.poll() is None, "login did not request approval"
                    line = proc.stderr.readline()
                url = re.search(r"at (\S+) using code", line)[1]
                code = line.strip().rsplit(" ", 1)[1]
                return proc, urllib.parse.urlsplit(url).path, code

            proc, path, code = start_login("login", "--no-browser", "--device-name", "Acceptance laptop")
            html = page(path)
            assert code in html and "Acceptance laptop" in html
            # GET never approves, and a real pending long poll must remain pending.
            time.sleep(6)
            assert proc.poll() is None
            browser_decision(path, "Acceptance laptop")
            stdout, stderr = proc.communicate(timeout=30)
            assert proc.returncode == 0, stderr
            assert json.loads(stdout) == {"logged_in": True, "expires_in": 2592000}
            records = list((home / ".config/bootleg/credentials").glob("*.json"))
            assert len(records) == 1 and records[0].stat().st_mode & 0o777 == 0o600
            token = json.loads(records[0].read_text())["token"]
            assert token not in stdout + stderr
            cli("whoami")
            results["chromium_login"] = "passed: actual CLI and Chromium"
            orgs = cli("orgs", "list")
            assert len(orgs) == 1
            org = orgs[0]["org_id"]
            token_file = home / "project-token"
            cli("projects", "create", "Acceptance project", "--org-id", org,
                          "--token-file", str(token_file))
            assert token_file.stat().st_mode & 0o777 == 0o600
            projects = cli("projects", "list", "--org-id", org)
            assert len(projects) == 1

            def configure(approve=False, cancel=False, multiple=False):
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
                proc = subprocess.Popen(base + ["config"], cwd=home, env=env, stdin=slave, stderr=slave,
                                        stdout=subprocess.PIPE, start_new_session=True)
                children.append(proc)
                os.close(slave)
                transcript = b""
                approved = selected_org = selected_project = False
                org_query_offset = None
                deadline = time.monotonic() + 45
                try:
                    while proc.poll() is None and time.monotonic() < deadline:
                        if select.select([master], [], [], .1)[0]:
                            try:
                                chunk = os.read(master, 65536)
                                transcript += chunk
                                if b"\x1b[6n" in chunk:
                                    os.write(master, b"\x1b[1;1R")
                            except OSError:
                                break
                        text = transcript.decode(errors="replace")
                        if approve and not approved:
                            match = re.search(r"at (http://\S+) using code", text)
                            if match:
                                path = urllib.parse.urlsplit(match[1]).path
                                html = page(path)
                                browser_decision(path, "Config laptop")
                                approved = True
                        if multiple and not selected_org and "Organization>" in text:
                            if org_query_offset is None:
                                org_query_offset = len(transcript)
                                os.write(master, b"Personal")
                            elif b"1/2" in transcript[org_query_offset:]:
                                # Drain PTY while filtering; fixed sleeps can block fzf's output.
                                os.write(master, b"\r")
                                selected_org = True
                        if not selected_project and "Project>" in text:
                            time.sleep(.3)
                            os.write(master, b"\x1b" if cancel else b"\r")
                            selected_project = True
                    assert proc.poll() is not None, repr(transcript)
                    stdout, _ = proc.communicate(timeout=3)
                    assert (proc.returncode == 0) != cancel, text
                    assert selected_project, text
                    assert not approve or (approved and "Could not open a browser automatically" in text)
                    assert token not in stdout.decode() + text
                    return json.loads(stdout)
                finally:
                    os.close(master)

            config = configure()
            assert config == {"api_url": origin + "/", "org_id": org, "project_id": projects[0]["project_id"]}
            saved = (home / ".bootleg.json").read_bytes()
            configure(cancel=True)
            assert (home / ".bootleg.json").read_bytes() == saved
            results["interactive_config"] = "passed: actual CLI and fzf PTY"
            presence_browser = subprocess.Popen(["node", str(ROOT / "scripts/presence-browser.cjs")],
                                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                                stderr=log, text=True)
            children.append(presence_browser)

            def browser_check(action, **fields):
                presence_browser.stdin.write(json.dumps(dict(action=action, **fields)) + "\n")
                presence_browser.stdin.flush()
                assert select.select([presence_browser.stdout], [], [], 40)[0], "browser response deadline"
                response = presence_browser.stdout.readline()
                assert response and json.loads(response).get("ok"), "browser presence check failed: " + action

            browser_check("open", origin=origin, path=f"/app/projects/{org}/{projects[0]['project_id']}",
                          cookies=[dict(name=c.name, value=c.value) for c in cookies])
            # Real native smuggler processes: duplicate labels and distinct identities.
            project_id = projects[0]["project_id"]
            # Agents receive only their own credential, never web signing keys or DB access.
            agent_env = dict({key: env[key] for key in ("PATH", "HOME", "TERM")},
                             BOOTLEG_API_URL=origin, BOOTLEG_ALLOW_INSECURE="true",
                             BOOTLEG_PROJECT_ID=project_id,
                             BOOTLEG_PROJECT_TOKEN=token_file.read_text().strip())

            def agent(dirty=None):
                settings = dict(agent_env)
                if dirty is not None:
                    settings.update(BOOTLEG_BUILD_COMMIT="a" * 40, BOOTLEG_BUILD_DIRTY="true")
                marker = home / f"application-{len(children)}.tick"
                proc = subprocess.Popen([str(SMUGGLER / "target/debug/smuggler"),
                                         "--name", "duplicate <b>label</b>", "--", "/bin/sh", "-c",
                                         'while :; do date +%s > "$1"; sleep 1; done', "application", str(marker)],
                                        env=settings, cwd=home, stdout=log, stderr=log)
                proc.application_marker = marker
                children.append(proc)
                return proc

            def application_advances(proc):
                before = proc.application_marker.stat().st_mtime_ns
                time.sleep(2)
                assert proc.poll() is None and proc.application_marker.stat().st_mtime_ns > before, "supervised application stopped"

            def wait_instances(predicate, seconds=25):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    items = [i["container"] for i in cli("instances", "list")["items"]]
                    if predicate(items):
                        return items
                    time.sleep(.5)
                counts = {state: sum(x["presence"] == state for x in items) for state in ("connected", "offline")}
                raise AssertionError(f"agent discovery deadline exceeded: {counts}")

            first = agent()
            second = agent(dirty=True)
            items = wait_instances(lambda xs: len(xs) == 2 and all(x["presence"] == "connected" for x in xs))
            assert len({x["container_id"] for x in items}) == 2
            assert {x["build_dirty"] for x in items} == {None, True}
            browser_check("rows", count=2, connected=2, label="duplicate <b>label</b>")
            results["native_discovery"] = "passed: native executables and CLI"
            results["browser_sse"] = "passed: actual Chromium, real SSE, text-only labels"
            # Kill only our disposable control plane: close actual sockets, not synthetic events.
            app.kill()
            app.wait(timeout=10)
            browser_check("stale", count=2)
            application_advances(first)
            application_advances(second)
            app = subprocess.Popen([str(WEB / "target/debug/bootleg-web"), "serve"],
                                   cwd=WEB, env=env, stdout=log, stderr=log)
            children.append(app)
            browser_check("live")
            wait_instances(lambda xs: len(xs) == 2 and all(x["presence"] == "connected" for x in xs), 65)
            results["control_plane_outage"] = "passed: real server killed/restarted, application progress and stale browser data"
            first.terminate()
            first.wait(timeout=10)
            agent()
            wait_instances(lambda xs: len(xs) == 3 and sum(x["presence"] == "connected" for x in xs) == 2, 55)
            browser_check("rows", count=3, connected=2)
            results["restart_identity"] = "passed: native executable restart"
            if options.docker:
                # Build an allowlisted context, never send sibling credentials/.git to Docker.
                context = home / "docker-context"
                for repo, paths in ((ROOT.parent / "bootleg-core", ("Cargo.toml", "src", "fixtures")),
                                    (SMUGGLER, ("Cargo.toml", "Cargo.lock", "src", "tests", "Dockerfile"))):
                    for relative in paths:
                        source = repo / relative
                        destination = context / repo.name / relative
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        if source.is_dir():
                            shutil.copytree(source, destination)
                        else:
                            shutil.copy2(source, destination)
                docker_image = resource_prefix + ":agent"
                run("docker", "build", "-q", "-t", docker_image, "-f",
                    str(context / "bootleg-smuggler/Dockerfile"), str(context))
                docker_network = resource_prefix + "-network"
                run("docker", "network", "create", docker_network)
                docker_env = {key: value for key, value in agent_env.items() if key.startswith("BOOTLEG_")}
                docker_env.update(BOOTLEG_API_URL=origin.replace("127.0.0.1", "host.docker.internal"),
                                  BOOTLEG_INSTANCE_NAME="Docker <b>agent</b>")
                args = ["docker", "run", "-d", "--name", resource_prefix + "-agent",
                        "--network", docker_network]
                for key in docker_env:
                    args.extend(("-e", key))
                docker_agent = run(*args, docker_image, env=dict(os.environ, **docker_env))
                containers.append(docker_agent)
                docker_items = wait_instances(lambda xs: len(xs) == 4 and sum(x["presence"] == "connected" for x in xs) == 3)
                before = next(x for x in docker_items if x["display_name"] == "Docker <b>agent</b>")
                browser_check("rows", count=4, connected=3, label="Docker <b>agent</b>")
                disconnected_at = time.monotonic()
                run("docker", "network", "disconnect", docker_network, docker_agent)
                wait_instances(lambda xs: all(x["presence"] == "offline" for x in xs
                                              if x["container_id"] == before["container_id"]), 50)
                assert time.monotonic() - disconnected_at <= 55, "offline exceeded 45-second lease plus 10-second observation allowance"
                browser_check("rows", count=4, connected=2)
                assert run("docker", "inspect", "--format", "{{.State.Running}}", docker_agent) == "true"
                run("docker", "network", "connect", docker_network, docker_agent)
                resumed = wait_instances(lambda xs: any(x["container_id"] == before["container_id"] and
                    x["presence"] == "connected" and x["smuggler_session_id"] != before["smuggler_session_id"] for x in xs), 65)
                assert len(resumed) == 4
                browser_check("rows", count=4, connected=3)
                results["docker_network_lifecycle"] = "passed: real Docker disconnect/reconnect, CLI and Chromium"
                print("PASS [Docker agent + CLI + Chromium]: discovery, network-loss expiry, reconnect identity/session")
            else:
                results["docker_network_lifecycle"] = "skipped: requires --docker on macOS Docker Desktop"
                print("SKIP: Docker network lifecycle (enable --docker on macOS Docker Desktop)")
            retained_count = 4 if options.docker else 3
            rotated = home / "rotated-token"
            cli("project", "token", "rotate", "--confirm", "--token-file", str(rotated))
            assert rotated.read_bytes() != token_file.read_bytes()
            wait_instances(lambda xs: len(xs) == retained_count and all(x["presence"] == "offline" for x in xs), 65)
            browser_check("rows", count=retained_count, connected=0)
            application_advances(second)
            agent()  # Old token cannot register a new container.
            time.sleep(3)
            assert len(cli("instances", "list")["items"]) == retained_count
            results["rotation"] = "passed: actual CLI, agents and browser"
            # Restore participation with the explicitly rotated credential, then deactivate
            # only this disposable project. Deactivation does not remove read visibility.
            agent_env["BOOTLEG_PROJECT_TOKEN"] = rotated.read_text().strip()
            authorized_agent = agent()
            retained_count += 1
            wait_instances(lambda xs: len(xs) == retained_count and sum(x["presence"] == "connected" for x in xs) == 1)
            browser_check("rows", count=retained_count, connected=1)
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                f"UPDATE project SET is_active=false WHERE id='{project_id}'")
            agent()  # Current token still cannot register against an inactive project.
            wait_instances(lambda xs: len(xs) == retained_count and all(x["presence"] == "offline" for x in xs), 25)
            browser_check("rows", count=retained_count, connected=0)
            application_advances(authorized_agent)
            results["deactivation"] = "passed: disposable DB change, actual agents/CLI/browser"
            # Revoke this account's eligibility while an established browser stream retains data.
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                "UPDATE users SET is_active=false WHERE email='cli-test@example.test'")
            browser_check("denied")
            results["browser_permission_loss"] = "passed: disposable account eligibility revoked, retained rows cleared"
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                "UPDATE users SET is_active=true WHERE email='cli-test@example.test'")
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                f"UPDATE project SET is_active=true WHERE id='{project_id}'")
            wait_instances(lambda xs: len(xs) == retained_count and all(x["presence"] == "offline" for x in xs))
            browser_check("close")
            cli("projects", "create", "Second project", "--org-id", org,
                "--token-file", str(home / "second-token"))
            # Multiple organizations trigger the other real fzf picker.
            html = page("/app")
            page("/organizations", dict(csrf=csrf(html), name="Another team", url=""))
            configure(multiple=True)
            results["multi_org_project_config"] = "passed: actual CLI and fzf PTY"
            # Expire credential in disposable DB: config must recover via fresh browser login.
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                "UPDATE cli_credential SET expires_at=now()")
            assert cli("whoami", ok=False)["error"]["code"] == "unauthorized"
            configure(approve=True, multiple=True)
            current_token = json.loads(records[0].read_text())["token"]
            cli("logout")
            env["BOOTLEG_USER_TOKEN"] = current_token
            env["BOOTLEG_USER_TOKEN_ORIGIN"] = origin
            assert cli("whoami", ok=False)["error"]["code"] == "unauthorized"
            del env["BOOTLEG_USER_TOKEN"]
            del env["BOOTLEG_USER_TOKEN_ORIGIN"]
            assert not list((home / ".config/bootleg/credentials").glob("*.json"))
            assert cli("whoami", ok=False)["error"]["code"] == "cli_error"
            # Missing credentials also initiate login inside config.
            configure(approve=True, multiple=True)
            cli("logout")
            proc, path, _ = start_login("login", "--no-browser")
            html = page(path)
            browser_decision(path, "Denied laptop", "deny")
            stdout, _ = proc.communicate(timeout=30)
            assert proc.returncode != 0 and json.loads(stdout)["error"]["code"] == "access_denied"
            results["expiry_logout_denial"] = "passed: actual CLI and Chromium"
            print("PASS [actual CLI + Chromium]: approval/denial, long poll, private credentials, expiry, logout")
            print("PASS [actual CLI + PTY]: config, cancellation, multiple organizations/projects, create, rotate")
            print("PASS [actual native agents + CLI]: duplicate labels, unknown/dirty baselines, restart identity,")
            print("     discovery, rotation rejection and application survival after authorization loss")
            print("PASS [Chromium + real SSE]: live updates without refresh, literal markup labels,")
            print("     control-plane outage retains stale data and reconnects; agent supervisors survive")
            print("PASS [agents + CLI + Chromium]: project deactivation, application survival, permission-loss clearing")
            print("SKIP: kind discovery; cross-org isolation; stale-session fencing;")
            print("      disabled network; browser hidden tabs")
            print("Registration acceptance remains INCOMPLETE; diff preview gate is closed.")
        finally:
            failure = sys.exc_info()[0]
            for child in children:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
            cleanup_failed = False
            for cid in containers:
                result = subprocess.run(["docker", "rm", "-f", "-v", cid],
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                cleanup_failed |= result.returncode != 0
            if docker_network:
                cleanup_failed |= subprocess.run(["docker", "network", "rm", docker_network],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
            if docker_image:
                cleanup_failed |= subprocess.run(["docker", "image", "rm", docker_image],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
            log.close() if "log" in locals() else None
            # Allowlisted diagnostic report: never persist raw exceptions, logs, cookies or DTOs.
            report = dict(scenarios=results, registration_complete=False,
                          failure_type=failure.__name__ if failure else None,
                          cleanup="failed" if cleanup_failed else "completed",
                          revisions={repo.name: run("git", "rev-parse", "HEAD", cwd=repo)
                                     for repo in (ROOT, WEB, SMUGGLER, ROOT.parent / "bootleg-core")})
            report_path = artifact_dir / "report.json"
            with open(report_path, "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
                json.dump(report, output, indent=2)
            print(f"Sanitized scenario report: {report_path}")
            if cleanup_failed:
                raise RuntimeError(f"Resource cleanup failed; inspect only resources named {resource_prefix}-*")
            print("CLEANUP: owned processes stopped and disposable store containers/volumes removed")


if __name__ == "__main__":
    main()
