#!/usr/bin/env python3
"""Real CLI/web acceptance test; disposable Docker stores, no API mocks.
Requires Python 3, Docker, fzf, Node, and sibling web Playwright/Chromium.
Account fixtures use HTTP forms; device approval uses actual Chromium UI.
"""
import base64
import signal
import uuid
import fcntl
import struct
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
    assert shutil.which("fzf"), "install fzf"
    run("cargo", "build", "--locked", cwd=ROOT)
    run("cargo", "build", "--locked", "-p", "bootleg-web", cwd=WEB)
    run("cargo", "build", "--locked", cwd=SMUGGLER)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    resource_prefix = "bootleg-acceptance-" + uuid.uuid4().hex[:12]
    containers = []
    children = []
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
                            time.sleep(.3)
                            os.write(master, b"Personal")
                            time.sleep(.5)  # Allow fzf's asynchronous filtering before Enter.
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
            # Real native smuggler processes: duplicate labels and distinct identities.
            project_id = projects[0]["project_id"]
            agent_env = dict(env, BOOTLEG_API_URL=origin, BOOTLEG_ALLOW_INSECURE="true",
                             BOOTLEG_PROJECT_ID=project_id,
                             BOOTLEG_PROJECT_TOKEN=token_file.read_text().strip())

            def agent(dirty=None):
                settings = dict(agent_env)
                if dirty is not None:
                    settings.update(BOOTLEG_BUILD_COMMIT="a" * 40, BOOTLEG_BUILD_DIRTY="true")
                proc = subprocess.Popen([str(SMUGGLER / "target/debug/smuggler"),
                                         "--name", "duplicate <b>label</b>", "--", "/bin/sleep", "300"],
                                        env=settings, cwd=home, stdout=log, stderr=log)
                children.append(proc)
                return proc

            def wait_instances(predicate, seconds=25):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    items = [i["container"] for i in cli("instances", "list")["items"]]
                    if predicate(items):
                        return items
                    time.sleep(.5)
                raise AssertionError("native agent discovery deadline exceeded")

            first = agent()
            second = agent(dirty=True)
            items = wait_instances(lambda xs: len(xs) == 2 and all(x["presence"] == "connected" for x in xs))
            assert len({x["container_id"] for x in items}) == 2
            assert {x["build_dirty"] for x in items} == {None, True}
            first.terminate()
            first.wait(timeout=10)
            agent()
            wait_instances(lambda xs: len(xs) == 3 and sum(x["presence"] == "connected" for x in xs) == 2)
            rotated = home / "rotated-token"
            cli("project", "token", "rotate", "--confirm", "--token-file", str(rotated))
            assert rotated.read_bytes() != token_file.read_bytes()
            wait_instances(lambda xs: len(xs) == 3 and all(x["presence"] == "offline" for x in xs), 65)
            assert second.poll() is None, "authorization loss terminated supervised application"
            agent()  # Old token cannot register a new container.
            time.sleep(3)
            assert len(cli("instances", "list")["items"]) == 3
            cli("projects", "create", "Second project", "--org-id", org,
                "--token-file", str(home / "second-token"))
            # Multiple organizations trigger the other real fzf picker.
            html = page("/app")
            page("/organizations", dict(csrf=csrf(html), name="Another team", url=""))
            configure(multiple=True)
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
            print("PASS [actual CLI + Chromium]: approval/denial, long poll, private credentials, expiry, logout")
            print("PASS [actual CLI + PTY]: config, cancellation, multiple organizations/projects, create, rotate")
            print("PASS [actual native agents + CLI]: duplicate labels, unknown/dirty baselines, restart identity,")
            print("     discovery, rotation rejection and application survival after authorization loss")
            print("SKIP: Docker/kind discovery; cross-org isolation; network interruption/reconnect;")
            print("      stale-session fencing; deactivation; outage; disabled network; browser presence/SSE")
            print("Registration acceptance remains INCOMPLETE; diff preview gate is closed.")
        finally:
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
            log.close() if "log" in locals() else None
            if cleanup_failed:
                raise RuntimeError(f"Resource cleanup failed; inspect only resources named {resource_prefix}-*")
            print("CLEANUP: owned processes stopped and disposable store containers/volumes removed")


if __name__ == "__main__":
    main()
