"""Drop committed real-web responses; never synthesize API responses."""
from contextlib import nullcontext
import http.client
import http.server
import json
import socket
import subprocess
import threading
import urllib.parse


class CommitDropProxy:
    def __init__(self, upstream):
        target = urllib.parse.urlsplit(upstream)
        self.path = None
        self.dropped = 0
        self.after_drop = 0
        self.failed = False
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass  # Requests and responses may contain credentials.

            def do_GET(self):
                self.forward()

            def do_POST(self):
                self.forward()

            def forward(self):
                connection = http.client.HTTPConnection(target.hostname, target.port, timeout=35)
                try:
                    selected = self.command == "POST" and self.path == owner.path
                    if selected and owner.dropped:
                        owner.after_drop += 1
                    body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                    headers = {k: v for k, v in self.headers.items()
                               if k.lower() not in ("host", "connection", "transfer-encoding")}
                    headers["Host"] = target.netloc
                    connection.request(self.command, self.path, body, headers)
                    response = connection.getresponse()
                    payload = response.read()  # Actual server completed; nothing sent downstream yet.
                    if selected and 200 <= response.status < 300:
                        owner.dropped += 1
                        self.close_connection = True
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                    self.send_response(response.status)
                    for k, v in response.getheaders():
                        if k.lower() not in ("connection", "transfer-encoding", "content-length"):
                            self.send_header(k, v)
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except Exception:
                    owner.failed = True
                    self.close_connection = True
                finally:
                    connection.close()

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = False
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def arm(self, path):
        self.path = "/api/v1/" + path
        self.dropped = self.after_drop = 0
        self.failed = False

    def check(self):
        assert not self.failed and self.dropped == 1 and self.after_drop == 0

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=40)
        assert not self.thread.is_alive()


def exercise(origin, base, env, home, token, org, project, pg, cli,
             start_login, browser_decision, results, proxy):
    def scalar(query):
        return subprocess.check_output(
            ["docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-Atc", query],
            text=True).strip()

    original = base[2]
    with nullcontext(proxy):
        base[2] = proxy.origin
        settings = dict(env, BOOTLEG_USER_TOKEN=token, BOOTLEG_USER_TOKEN_ORIGIN=proxy.origin)
        try:
            proxy.arm(f"orgs/{org}/projects")
            output = cli("projects", "create", "Ambiguous creation", "--org-id", org,
                         "--token-file", str(home / "ambiguous-create-token"), ok=False, settings=settings)
            assert "Check server state before retrying" in json.dumps(output)
            proxy.check()
            assert scalar("SELECT count(*) FROM project WHERE name='Ambiguous creation'") == "1"
            assert not (home / "ambiguous-create-token").exists()
            results["ambiguous_creation"] = "passed: actual CLI/web/stores; committed response dropped; one creation and no retry"

            before = int(scalar(f"SELECT count(*) FROM activity_event WHERE project_id='{project}' AND kind='token.rotated'"))
            proxy.arm(f"projects/{project}/token/rotate")
            output = cli("project", "token", "rotate", "--project-id", project, "--confirm", "--token-file", str(home / "ambiguous-rotate-token"),
                         ok=False, settings=settings)
            assert "Check server state before retrying" in json.dumps(output)
            proxy.check()
            after = int(scalar(f"SELECT count(*) FROM activity_event WHERE project_id='{project}' AND kind='token.rotated'"))
            assert after == before + 1
            assert not (home / "ambiguous-rotate-token").exists()
            results["ambiguous_rotation"] = "passed: actual CLI/web/stores; committed response dropped; one rotation and no retry"

            login_home = home / "ambiguous-login"
            login_home.mkdir(mode=0o700)
            login_env = dict(env, HOME=str(login_home))
            login_env.pop("BOOTLEG_USER_TOKEN", None)
            login_env.pop("BOOTLEG_USER_TOKEN_ORIGIN", None)
            proxy.arm("auth/device/token")
            proc, path, _ = start_login("login", "--no-browser", "--device-name", "Ambiguous login",
                                       settings=login_env, directory=login_home)
            browser_decision(path, "Ambiguous login")
            stdout, stderr = proc.communicate(timeout=40)
            assert proc.returncode != 0
            assert "Revoke this device" in stdout + stderr and "fresh" in stdout + stderr
            proxy.check()
            assert scalar("SELECT count(*) FROM cli_device_grant WHERE name='Ambiguous login' AND state='exchanged'") == "1"
            assert scalar("SELECT count(*) FROM cli_credential WHERE name='Ambiguous login'") == "1"
            assert not list(login_home.glob(".config/bootleg/credentials/*.json"))
            results["ambiguous_device_exchange"] = "passed: actual CLI/Chromium/web/stores; committed response dropped; one credential, no retry, fresh-login guidance"
        finally:
            base[2] = original
