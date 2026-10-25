"""Linux syscall evidence for disabled mode, with an enabled positive control.

No packet-count inference from --network none: strace observes attempted network
syscalls across the actual smuggler/application process tree. Neither case has
network access. The enabled case must attempt network I/O for the test to pass.
"""
import os
import re
import subprocess


def verify_disabled(image, trace_image, prefix, home, settings, containers, log):
    context = home / "trace-image"
    context.mkdir(mode=0o700)
    (context / "Dockerfile").write_text(
        f"FROM {image}\nUSER root\n"
        "RUN apt-get update && apt-get install -y --no-install-recommends strace "
        "&& rm -rf /var/lib/apt/lists/*\n")
    subprocess.run(["docker", "build", "-q", "-t", trace_image, str(context)],
                   stdout=log, stderr=log, check=True, timeout=300)
    for disabled in (False, True):
        name = prefix + ("-disabled" if disabled else "-network-control")
        environment = dict(settings, BOOTLEG_DISABLED=str(disabled).lower(),
                           BOOTLEG_API_URL="http://127.0.0.1:9")
        args = ["docker", "create", "--name", name, "--network", "none",
                "--cap-add", "SYS_PTRACE", "--entrypoint", "/usr/bin/strace"]
        for key in environment:
            args.extend(("-e", key))
        args.extend((trace_image, "-f", "-e", "trace=network", "-o", "/network.trace",
                     "/usr/local/bin/smuggler", "--", "/bin/sh", "-c", "sleep 3; exit 23"))
        cid = subprocess.check_output(args, env=dict(os.environ, **environment), text=True).strip()
        containers.append(cid)
        subprocess.run(["docker", "start", "-a", cid], stdout=log, stderr=log, timeout=30)
        status = subprocess.check_output(["docker", "inspect", "--format", "{{.State.ExitCode}}", cid], text=True).strip()
        assert status == "23", "traced application did not preserve its exit status"
        trace = home / (name + ".trace")
        subprocess.run(["docker", "cp", cid + ":/network.trace", str(trace)], check=True,
                       stdout=log, stderr=log, timeout=15)
        # With trace=network, any syscall entry is a network attempt, even if it failed.
        calls = re.findall(r"\b[a-z][a-z0-9_]*\(", trace.read_text())
        assert bool(calls) != disabled, "disabled-mode network syscall assertion failed"
