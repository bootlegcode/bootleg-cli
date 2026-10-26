#!/usr/bin/env python3
"""Build allowlisted sources; trace local capture in an isolated Linux container."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]


def main():
    name = "bootleg-preview-" + uuid.uuid4().hex[:12]
    image = name + ":syscalls"
    try:
        with tempfile.TemporaryDirectory(prefix=name) as temporary:
            context = Path(temporary)
            for repo, paths in [(ROOT.parent / "bootleg-core", ("Cargo.toml", "src", "fixtures")),
                                (ROOT, ("Cargo.toml", "Cargo.lock", "src", "examples"))]:
                for relative in paths:
                    source = repo / relative
                    destination = context / repo.name / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    if source.is_dir():
                        shutil.copytree(source, destination)
                    else:
                        shutil.copy2(source, destination)
            shutil.copy2(ROOT / "scripts/preview_syscall_fixture.py", context / "fixture.py")
            (context / "Dockerfile").write_text("""FROM rust:1.94-bookworm AS build
WORKDIR /build
COPY bootleg-core bootleg-core
COPY bootleg-cli bootleg-cli
WORKDIR /build/bootleg-cli
RUN cargo build --locked --example preview_capture_probe
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git strace python3 && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/bootleg-cli/target/debug/examples/preview_capture_probe /usr/local/bin/preview_capture_probe
COPY fixture.py /fixture.py
ENTRYPOINT ["python3", "/fixture.py"]
""")
            subprocess.run(["docker", "build", "-q", "-t", image, str(context)], check=True, timeout=1200)
            run = subprocess.run(["docker", "run", "--name", name, "--rm", "--read-only", "--network", "none",
                                  "--tmpfs", "/work:rw,nosuid,nodev,size=64m", "--cap-add=SYS_PTRACE", image],
                                 capture_output=True, text=True, timeout=300)
            if run.returncode:
                raise RuntimeError(run.stderr)
            report = json.loads(run.stdout)
            path = ROOT / "docs/diff-preview/syscalls.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
            print("PASS: isolated Linux capture syscall proof; " + str(path))
    finally:
        subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["docker", "image", "rm", image], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
