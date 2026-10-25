"""Disposable acceptance-load orchestration; no development service mutation."""
import json
import os
import platform
import socket
import subprocess
import threading
import time
import urllib.request


def measure(config, origin, env, home, log, children, pg, org, project,
            project_token, user_token, cookies, web, root):
    origins = [origin]
    for _ in range(config["replicas"] - 1):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{sock.getsockname()[1]}"
        settings = dict(env, BIND_ADDR=address)
        proc = subprocess.Popen([str(web / "target/debug/bootleg-web"), "serve"],
                                cwd=web, env=settings, stdout=log, stderr=log)
        children.append(proc)
        replica = "http://" + address
        for attempt in range(100):
            try:
                urllib.request.urlopen(replica + "/health/ready", timeout=1).close()
                break
            except OSError:
                if proc.poll() is not None or attempt == 99:
                    raise RuntimeError("load replica startup failed")
                time.sleep(.2)
        origins.append(replica)
    samples = []
    sampler_errors = []
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            try:
                result = subprocess.check_output([
                    "docker", "exec", pg, "psql", "-U", "postgres", "-d", "cli_test", "-At", "-c",
                    "SELECT count(*), count(*) FILTER (WHERE state='active'), "
                    "count(*) FILTER (WHERE wait_event='advisory'), "
                    "coalesce(max(extract(epoch FROM clock_timestamp()-query_start)) "
                    "FILTER (WHERE wait_event='advisory'),0) "
                    "FROM pg_stat_activity WHERE datname='cli_test' AND pid<>pg_backend_pid()"],
                    text=True, stderr=subprocess.DEVNULL, timeout=5)
                samples.append([float(x) for x in result.strip().split("|")])
            except (subprocess.SubprocessError, ValueError):
                sampler_errors.append(1)
            stop.wait(.5)

    thread = threading.Thread(target=sample)
    thread.start()
    started = time.monotonic()
    try:
        driver = subprocess.Popen(["node", str(root / "scripts/presence-load.cjs")],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        children.append(driver)
        output, _ = driver.communicate(json.dumps(dict(config, origins=origins, org=org, project=project,
            projectToken=project_token, userToken=user_token,
            cookie="; ".join(f"{c.name}={c.value}" for c in cookies))),
            timeout=config["seconds"] + config["agents"] * .1 + 180)
        result = json.loads(output)
    finally:
        stop.set()
        thread.join()
    result["elapsed_seconds"] = time.monotonic() - started
    result["postgres_samples"] = dict(columns=["connections", "active", "advisory_waiters", "oldest_waiting_query_seconds"],
                                       interval_seconds=.5, rows=samples, failures=len(sampler_errors))
    if sampler_errors:
        result.setdefault("errors", {})["postgres_sampler"] = len(sampler_errors)
    result["environment"] = dict(os=platform.platform(), machine=platform.machine(), cpus=os.cpu_count(),
                                 profile="debug", pool_max_per_replica=int(env.get("PGPOOL_MAX_CONNECTIONS", "10")))
    for name, command in {"rust": ["rustc", "--version"], "node": ["node", "--version"],
                          "docker": ["docker", "version", "--format", "{{.Server.Version}}"],
                          "postgres": ["docker", "exec", pg, "postgres", "--version"]}.items():
        result["environment"][name] = subprocess.check_output(command, text=True).strip()
    return result
