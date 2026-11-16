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
import traceback
import urllib.parse
import urllib.request

from acceptance_kind import KindAcceptance
from acceptance_network import verify_disabled
from acceptance_pty import finish_process, read_pty

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT.parent / "bootleg-web"
SMUGGLER = ROOT.parent / "bootleg-smuggler"
BIN = ROOT / "target/debug/bootleg"


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def spawn(*args, **kwargs):
    # Own the complete tree, including Chromium descendants on browser timeout.
    kwargs.setdefault("start_new_session", True)
    return subprocess.Popen(*args, **kwargs)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class InjectedAcceptanceFailure(RuntimeError):
    """Deliberate failure after all resource classes have been provisioned."""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--docker", action="store_true", help="exercise isolated Docker networking (Linux or macOS Docker Desktop)")
    parser.add_argument("--fail-after-kind", action="store_true", help="inject failure after kind discovery to verify cleanup")
    parser.add_argument("--mutation-failures", action="store_true", help="run committed-response-loss scenarios only")
    parser.add_argument("--fail-at", choices=("startup", "browser-timeout", "sigint", "sigterm", "docker-partial", "kind-partial"))
    parser.add_argument("--kind", action="store_true", help="also create an isolated kind cluster (implies --docker)")
    parser.add_argument("--allow-dirty-dependencies", action="store_true", help="explicitly test tracked edits at the pinned sibling revisions")
    parser.add_argument("--presence-load", metavar="AGENTS,VIEWERS,REPLICAS,SECONDS",
                        help="run isolated protocol/SSE capacity measurement instead of registration scenarios")
    options = parser.parse_args()
    load_config = None
    if options.presence_load:
        try:
            agents, viewers, replicas, seconds = map(int, options.presence_load.split(","))
            assert 1 <= agents <= 200 and 0 <= viewers <= 128 * replicas and 1 <= replicas <= 4 and seconds >= 90
            load_config = dict(agents=agents, viewers=viewers, replicas=replicas, seconds=seconds)
        except (ValueError, AssertionError):
            parser.error("load requires 1..200 agents, 0..128 viewers/replica, 1..4 replicas, >=90 seconds")
    pins = json.loads((ROOT / "scripts/acceptance-revisions.json").read_text())
    for name, expected in pins.items():
        repo = ROOT.parent / name
        assert run("git", "rev-parse", "HEAD", cwd=repo) == expected, f"{name}: checkout must match scripts/acceptance-revisions.json (no automatic checkout)"
        assert options.allow_dirty_dependencies or not run("git", "status", "--porcelain", "--untracked-files=no", cwd=repo), f"{name}: tracked edits require --allow-dirty-dependencies"
    if options.kind:
        options.docker = True
        assert shutil.which("kind") and shutil.which("kubectl"), "install kind and kubectl"
    if options.fail_at == "kind-partial" and not options.kind:
        parser.error("--fail-at kind-partial requires --kind")
    if options.fail_at == "docker-partial" and not options.docker:
        parser.error("--fail-at docker-partial requires --docker")
    if options.fail_after_kind and not options.kind:
        parser.error("--fail-after-kind requires --kind")
    if options.docker and os.uname().sysname not in ("Darwin", "Linux"):
        parser.error("--docker requires Linux or macOS Docker Desktop")
    assert shutil.which("fzf"), "install fzf"
    run("cargo", "nextest", "--version")  # supplemental real-store suite requires it
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
    if options.mutation_failures:
        results.update({name: "skipped: not reached" for name in (
            "ambiguous_creation", "ambiguous_rotation", "ambiguous_device_exchange")})
    containers = []
    children = []
    docker_image = docker_network = trace_image = None
    kind_cluster = None
    browser_failure_stage = None
    mutation_proxy = None
    injection_reached = False
    with tempfile.TemporaryDirectory(prefix="bootleg-cli-e2e-") as directory:
        home = Path(directory)
        env = {k: v for k, v in os.environ.items() if not k.startswith(
            ("BOOTLEG_", "PG", "DATABASE_URL", "VALKEY_", "OTEL_"))}
        env.update(HOME=directory, TERM="xterm-256color")
        try:
            def container(image, port, *args):
                name = f"{resource_prefix}-{len(containers)}"
                containers.append(name)  # Record intent before Docker can create anything.
                cid = run("docker", "run", "--name", name, "--rm", "-d", "-p", f"127.0.0.1::{port}", *args, image)
                address = run("docker", "port", cid, str(port)).splitlines()[0]
                return cid, address.rsplit(":", 1)[1]

            pg, pgport = container("postgres:18-alpine", 5432,
                                   "-e", "POSTGRES_PASSWORD=isolated-test", "-e", "POSTGRES_DB=cli_test")
            _, redisport = container("valkey/valkey:8-alpine", 6379)
            _, mailport = container("axllent/mailpit:latest", 1025)
            for _ in range(100):
                if subprocess.run(["docker", "exec", pg, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                    break
                time.sleep(.2)
            origin = f"http://127.0.0.1:{free_port()}"
            bind_address = origin.removeprefix("http://")
            if options.mutation_failures:
                from acceptance_mutations import CommitDropProxy
                mutation_proxy = CommitDropProxy(origin)
                mutation_proxy.__enter__()
                origin = mutation_proxy.origin
            env.update(PGHOST="127.0.0.1", PGPORT=pgport, PGUSER="postgres",
                       PGPASSWORD="isolated-test", PGDATABASE="cli_test", PGSSLMODE="disable",
                       VALKEY_URL=f"redis://127.0.0.1:{redisport}/0",
                       BIND_ADDR=bind_address, BOOTLEG_PUBLIC_BASE_URL=origin,
                       BOOTLEG_INSECURE_COOKIES="true", BOOTLEG_EMAIL_LOCAL_MODE="true",
                       BOOTLEG_EMAIL_ALLOW_LOCAL_HTTP="true", BOOTLEG_EMAIL_PROVIDER="smtp",
                       BOOTLEG_EMAIL_SMTP_SECURITY="local_plaintext", BOOTLEG_EMAIL_SMTP_HOST="127.0.0.1",
                       BOOTLEG_EMAIL_SMTP_PORT=mailport, BOOTLEG_EMAIL_FROM_ADDRESS="noreply@example.test",
                       BOOTLEG_EMAIL_VERIFICATION_SIGNING_KEY=base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip("="))
            run(str(WEB / "target/debug/bootleg-web"), "migrate", cwd=WEB, env=env)
            log = open(home / "web.log", "w+")
            startup_env = dict(env)
            if options.fail_at == "startup":
                startup_env["BIND_ADDR"] = "invalid bind address"
            app = spawn([str(WEB / "target/debug/bootleg-web"), "serve"],
                        cwd=WEB, env=startup_env, stdout=log, stderr=log)
            children.append(app)
            for _ in range(100):
                try:
                    urllib.request.urlopen(origin + "/health/ready", timeout=1).close()
                    break
                except OSError:
                    if app.poll() is not None:
                        injection_reached = options.fail_at == "startup"
                        log.seek(0)
                        raise AssertionError("web startup failed: " + log.read())
                    time.sleep(.2)
            cookies = http.cookiejar.CookieJar()
            browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))

            def browser_decision(path, name, decision="approve", jar=None):
                approval = spawn(["node", str(ROOT / "scripts/approve-device.cjs")],
                                            stdin=subprocess.PIPE, stdout=log, stderr=log, text=True)
                children.append(approval)
                approval.communicate(json.dumps(dict(origin=origin, path=path, name=name,
                    decision=decision, cookies=[dict(name=c.name, value=c.value)
                                                for c in (cookies if jar is None else jar)],
                    stall=options.fail_at == "browser-timeout",
                    stallMarker=str(home / "browser-stalled"))),
                    timeout=10 if options.fail_at == "browser-timeout" else 40)
                assert approval.returncode == 0, "Chromium device decision failed"

            def page(path, fields=None, opener=None):
                data = None if fields is None else urllib.parse.urlencode(fields).encode()
                with (opener or browser).open(origin + path, data=data, timeout=30) as response:
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

            def cli(*args, ok=True, settings=None, directory=None):
                result = subprocess.run(base + list(args), cwd=directory or home, env=settings or env, capture_output=True, text=True, timeout=35)
                assert (result.returncode == 0) == ok, result.stderr + result.stdout
                return json.loads(result.stdout)

            def start_login(*args, settings=None, directory=None):
                proc = spawn(base + list(args), cwd=directory or home, env=settings or env,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                children.append(proc)
                deadline = time.monotonic() + 30
                transcript = b""
                while True:
                    assert time.monotonic() < deadline, "login approval deadline"
                    if select.select([proc.stderr], [], [], .1)[0]:
                        chunk = os.read(proc.stderr.fileno(), 65536)
                        assert chunk, "login exited before requesting approval"
                        transcript += chunk
                    match = re.search(rb"Approve Bootleg CLI at (\S+) using code (\S+)\n", transcript)
                    if match:
                        url, code = (value.decode() for value in match.groups())
                        break
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

            if options.fail_at in ("sigint", "sigterm"):
                injection_reached = True
                os.kill(os.getpid(), signal.SIGINT if options.fail_at == "sigint" else signal.SIGTERM)
            if options.mutation_failures:
                from acceptance_mutations import exercise
                exercise(origin, base, env, home, token, org, projects[0]["project_id"],
                         pg, cli, start_login, browser_decision, results, mutation_proxy)
                return

            if load_config:
                from presence_load import measure
                results["presence_load"] = "running"
                measurement = measure(load_config, origin, env, home, log, children, pg,
                                      org, projects[0]["project_id"], token_file.read_text().strip(),
                                      token, cookies, WEB, ROOT)
                with open(artifact_dir / "load.json", "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
                    json.dump(measurement, output, indent=2)
                assert not measurement.get("failure") and not measurement["errors"], "load measurement failed; see sanitized load.json"
                results["presence_load"] = "passed: actual stores, web replicas, protocol sockets and HTTP SSE viewers"
                return

            # A second real account has no membership in the first account's organizations.
            outsider_home = home / "outsider"
            outsider_home.mkdir(mode=0o700)
            outsider_env = dict(env, HOME=str(outsider_home))
            outsider_cookies = http.cookiejar.CookieJar()
            outsider_browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(outsider_cookies))
            html = page("/register", opener=outsider_browser)
            page("/register", dict(csrf=csrf(html), email="outsider@example.test", password="test-only other password", timezone="UTC"), opener=outsider_browser)
            run("docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-c",
                "UPDATE users SET verified=true, email_verified_at=now(), email_verification_issued_at=NULL, email_verification_expires_at=NULL WHERE email='outsider@example.test'")
            proc, path, _ = start_login("login", "--no-browser", settings=outsider_env, directory=outsider_home)
            browser_decision(path, "Outsider laptop", jar=outsider_cookies)
            stdout, stderr = proc.communicate(timeout=30)
            assert proc.returncode == 0, "outsider login failed"

            def outsider(*args, **kwargs):
                return cli(*args, settings=outsider_env, directory=outsider_home, **kwargs)

            outsider_orgs = outsider("orgs", "list")
            assert len(outsider_orgs) == 1 and outsider_orgs[0]["org_id"] != org
            outsider_project = outsider("projects", "create", "Private outsider project", "--org-id",
                                       outsider_orgs[0]["org_id"], "--token-file", str(outsider_home / "project-token"))
            assert cli("instances", "list", "--project-id", outsider_project["project_id"], ok=False)["error"]["code"] == "not_found"
            assert outsider("instances", "list", "--project-id", projects[0]["project_id"], ok=False)["error"]["code"] == "not_found"
            assert outsider("project", "token", "rotate", "--project-id", projects[0]["project_id"],
                            "--confirm", "--token-file", str(outsider_home / "forbidden-token"), ok=False)["error"]["code"] == "not_found"
            outsider("logout")
            results["cross_org_isolation"] = "passed: two actual CLI/browser logins, disjoint orgs, bidirectional discovery and rotation denial"

            def configure(approve=False, cancel=False, multiple=False):
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
                proc = spawn(base + ["config"], cwd=home, env=env, stdin=slave, stderr=slave,
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
                            chunk = read_pty(master)
                            if not chunk:
                                break
                            transcript += chunk
                            if b"\x1b[6n" in chunk:
                                os.write(master, b"\x1b[1;1R")
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
                    # The slave may close before waitpid can observe process exit.
                    # Wait rather than asserting on a racing nonblocking poll.
                    stdout, _ = finish_process(proc, deadline)
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
            presence_browser = spawn(["node", str(ROOT / "scripts/presence-browser.cjs")],
                                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                                stderr=log, text=True)
            children.append(presence_browser)

            def browser_check(action, **fields):
                nonlocal browser_failure_stage
                presence_browser.stdin.write(json.dumps(dict(action=action, **fields)) + "\n")
                presence_browser.stdin.flush()
                assert select.select([presence_browser.stdout], [], [], 40)[0], "browser response deadline"
                response = presence_browser.stdout.readline()
                response = json.loads(response) if response else {}
                if not response.get("ok"):
                    stage = response.get("stage")
                    if stage in ("command", "chromium_start", "cdp_connect", "page_navigation", "initial_live"):
                        browser_failure_stage = stage
                assert response.get("ok"), "browser presence check failed: " + action

            browser_check("open", origin=origin, profile=str(home / "presence-chromium"),
                          path=f"/app/projects/{org}/{projects[0]['project_id']}",
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
                proc = spawn([str(SMUGGLER / "target/debug/smuggler"),
                                         "--name", "duplicate <img src=x onerror=alert(1)>", "--", "/bin/sh", "-c",
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
            browser_check("rows", count=2, connected=2, label="duplicate <img src=x onerror=alert(1)>", baselines=True)
            results["native_discovery"] = "passed: native executables and CLI"
            from acceptance_preview import verify_preview
            results["diff_preview"] = verify_preview(base, env, home, org,
                SMUGGLER / "target/debug/smuggler", children, log, outsider_env, pg)
            results["browser_sse"] = "passed: actual Chromium, real SSE, text-only labels"
            browser_check("hide")
            browser_check("show")
            results["hidden_tabs"] = "passed: native Chromium tab visibility, network-observed SSE close and reconnect"
            # Kill only our disposable control plane: close actual sockets, not synthetic events.
            app.kill()
            app.wait(timeout=10)
            browser_check("stale", count=2)
            application_advances(first)
            application_advances(second)
            app = spawn([str(WEB / "target/debug/bootleg-web"), "serve"],
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
                if options.fail_at == "docker-partial":
                    injection_reached = True
                    raise InjectedAcceptanceFailure("partial Docker provisioning")
                agent_origin = origin.replace("127.0.0.1", "host.docker.internal")
                if os.uname().sysname == "Linux":
                    # Native web/stores remain loopback-only. Expose only web through
                    # the owned bridge gateway, restricted to peers on that subnet.
                    ipam = json.loads(run("docker", "network", "inspect", docker_network))[0]["IPAM"]["Config"][0]
                    gateway, subnet = ipam["Gateway"], ipam["Subnet"]
                    port = urllib.parse.urlsplit(origin).port
                    relay = spawn([sys.executable, str(ROOT / "scripts/acceptance_relay.py"),
                                              gateway, subnet, str(port)], stdout=log, stderr=log)
                    children.append(relay)
                    for _ in range(100):
                        assert relay.poll() is None, "bridge relay exited"
                        try:
                            with socket.create_connection((gateway, port), timeout=.2):
                                break
                        except OSError:
                            time.sleep(.1)
                    else:
                        raise AssertionError("bridge relay unavailable")
                    agent_origin = origin.replace("127.0.0.1", gateway)
                docker_env = {key: value for key, value in agent_env.items() if key.startswith("BOOTLEG_")}
                docker_env.update(BOOTLEG_API_URL=agent_origin,
                                  BOOTLEG_INSTANCE_NAME="Docker <b>agent</b>")
                args = ["docker", "run", "-d", "--name", resource_prefix + "-agent",
                        "--network", docker_network]
                for key in docker_env:
                    args.extend(("-e", key))
                containers.append(resource_prefix + "-agent")
                docker_agent = run(*args, docker_image, env=dict(os.environ, **docker_env))
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
                trace_image = resource_prefix + ":trace"
                verify_disabled(docker_image, trace_image, resource_prefix, home, docker_env, containers, log)
                results["disabled_network"] = "passed: real Linux executable, strace network syscalls absent when disabled; enabled positive control observed"
                print("PASS [Linux executable + strace]: disabled no-network syscalls, enabled positive control, application exit status")
            else:
                results["docker_network_lifecycle"] = "skipped: requires --docker on Linux or macOS Docker Desktop"
                results["disabled_network"] = "skipped: requires --docker and Linux syscall tracing"
                print("SKIP: Docker network lifecycle (enable --docker on Linux or macOS Docker Desktop)")
            retained_count = 4 if options.docker else 3
            if options.kind:
                kind_cluster = KindAcceptance(resource_prefix, home, docker_network)
                pods = kind_cluster.start(docker_image, agent_origin, project_id, token_file,
                                          fail_after_secret=options.fail_at == "kind-partial")
                retained_count += 4
                snapshots = wait_instances(lambda xs: len(xs) == retained_count and
                                           sum(x["presence"] == "connected" for x in xs) == retained_count - 1, 65)
                deployed = [x for x in snapshots if (x["runtime"] or {}).get("cluster_name") == resource_prefix]
                assert len(deployed) == 4 and len({x["container_id"] for x in deployed}) == 4
                for uid, name in pods.items():
                    same_pod = [x for x in deployed if x["runtime"]["pod_uid"] == uid]
                    assert len(same_pod) == 2
                    assert {x["runtime"]["container_name"] for x in same_pod} == {"app", "worker"}
                    assert {x["runtime"]["pod_name"] for x in same_pod} == {name}
                    assert {x["display_name"] for x in same_pod} == {name + "/app", name + "/worker"}
                browser_check("rows", count=retained_count, connected=retained_count - 1)
                results["kind"] = "passed: isolated real kind cluster, two pods with app/worker agents each, CLI and Chromium discovery"
                print("PASS [kind + CLI + Chromium]: two replicas, distinct instrumented containers per pod")
                if options.fail_after_kind:
                    raise InjectedAcceptanceFailure("requested post-kind cleanup validation")
            else:
                results["kind"] = "skipped: requires --kind, kind and kubectl"
            rotated = home / "rotated-token"
            cli("project", "token", "rotate", "--confirm", "--token-file", str(rotated))
            assert rotated.read_bytes() != token_file.read_bytes()
            wait_instances(lambda xs: len(xs) == retained_count and all(x["presence"] == "offline" for x in xs), 25)
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
            # Supplement executable acceptance with real-store and protocol-client evidence.
            # The runner creates/drops only test_* databases in OUR PostgreSQL container and
            # flushes OUR Valkey. Stop our web process first; no browser/CLI work follows.
            app.kill()
            app.wait(timeout=10)
            store_env = {key: value for key, value in os.environ.items() if not key.startswith(
                ("BOOTLEG_", "PG", "DATABASE_URL", "MIGRATION_DATABASE_URL", "TEST_DATABASE", "VALKEY_", "OTEL_"))}
            store_env.update(BOOTLEG_TEST_ISOLATED="1",
                             TEST_DATABASE_ADMIN_URL=f"postgres://postgres:isolated-test@127.0.0.1:{pgport}/postgres",
                             VALKEY_URL=f"redis://127.0.0.1:{redisport}/0")
            subprocess.run(["cargo", "run", "--locked", "-p", "bootleg-infrastructure", "--example", "test_database", "--", "--presence"],
                           cwd=WEB, env=store_env, stdout=log, stderr=log, check=True, timeout=300)
            results["stale_session_callbacks"] = "passed: real-store domain adapters and real WebSocket protocol clients; not executable-agent evidence"
            print("PASS [actual CLI + Chromium]: approval/denial, long poll, private credentials, expiry, logout")
            print("PASS [actual CLI + PTY]: config, cancellation, multiple organizations/projects, create, rotate")
            print("PASS [actual native agents + CLI]: duplicate labels, unknown/dirty baselines, restart identity,")
            print("     discovery, rotation rejection and application survival after authorization loss")
            print("PASS [Chromium + real SSE]: live updates without refresh, literal markup labels,")
            print("     control-plane outage retains stale data and reconnects; agent supervisors survive")
            print("PASS [agents + CLI + Chromium]: project deactivation, application survival, permission-loss clearing")
            print("PASS [two CLI accounts + Chromium]: cross-org discovery and token-rotation isolation")
            if not options.kind:
                print("SKIP: kind discovery (enable --kind)")
            print("PASS [Chromium native visibility + CDP network observation]: hidden tab closes SSE, foreground reconnects")
            print("PASS [real-store domain + real WebSocket clients]: old-session heartbeat/disconnect fencing (supplemental)")
            if not options.docker:
                print("SKIP: disabled network syscall proof (enable --docker)")
            complete = all(value.startswith("passed:") for value in results.values())
            print("Registration acceptance: " + ("GREEN (evidence levels reported above)" if complete else "INCOMPLETE; optional prerequisites/scenarios skipped"))
        finally:
            failure = sys.exc_info()[0]
            disk_free_bytes = shutil.disk_usage(ROOT).free
            cleanup_failed = False
            if mutation_proxy:
                mutation_proxy.__exit__(None, None, None)
            for child in children:
                # A node parent can exit while Chromium descendants retain private
                # files. Terminate the owned group even if its leader has exited.
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    if child.poll() is None:
                        child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        child.kill()
                    child.wait(timeout=5)
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass
            groups = {int(value) for value in run("ps", "-axo", "pgid=").split()}
            processes_removed = all(child.poll() is not None and child.pid not in groups for child in children)
            for cid in containers:
                # Failed provisioning may not have created the intended name.
                existing = run("docker", "ps", "-a", "--no-trunc", "--format", "{{.ID}} {{.Names}}")
                if any(cid in line.split() for line in existing.splitlines()):
                    result = subprocess.run(["docker", "rm", "-f", "-v", cid],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    cleanup_failed |= result.returncode != 0
            if kind_cluster:
                cleanup_failed |= not kind_cluster.cleanup()
            if docker_network:
                cleanup_failed |= subprocess.run(["docker", "network", "rm", docker_network],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
            for image in (trace_image, docker_image):
                if image:
                    cleanup_failed |= subprocess.run(["docker", "image", "rm", image],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
            log.close() if "log" in locals() else None
            if options.fail_at == "browser-timeout":
                injection_reached = (home / "browser-stalled").exists()
            if options.fail_at == "kind-partial":
                injection_reached = bool(kind_cluster and kind_cluster.failure_category == "injected_after_secret")
            shutil.rmtree(home)
            private_files_removed = not home.exists()
            cleanup_failed |= not processes_removed or not private_files_removed
            # Allowlisted diagnostic report: never persist raw exceptions, logs, cookies or DTOs.
            report = dict(scenarios=results, registration_complete=not failure and not cleanup_failed and all(
                              value.startswith("passed:") for value in results.values()),
                          failure_type=failure.__name__ if failure else None,
                          failure_injection=options.fail_at,
                          injection_reached=injection_reached,
                          private_files_removed=private_files_removed,
                          processes_removed=processes_removed,
                          disk_free_bytes=disk_free_bytes,
                          kind_failure=kind_cluster.failure_category if kind_cluster else None,
                          browser_failure_stage=browser_failure_stage,
                          cleanup="failed" if cleanup_failed else "completed",
                          failure_locations=[dict(script=Path(frame.filename).name, line=frame.lineno)
                                             for frame in traceback.extract_tb(sys.exc_info()[2])
                                             if Path(frame.filename).parent == ROOT / "scripts"],
                          revisions={repo.name: run("git", "rev-parse", "HEAD", cwd=repo)
                                     for repo in (ROOT, WEB, SMUGGLER, ROOT.parent / "bootleg-core")},
                          tracked_edits={repo.name: bool(run("git", "status", "--porcelain", "--untracked-files=no", cwd=repo))
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
