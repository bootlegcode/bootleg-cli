"""Runs inside the owned, network-disabled read-only Linux acceptance container."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

os.environ.update(HOME="/work/home", GIT_CONFIG_NOSYSTEM="1", LC_ALL="C")
Path(os.environ["HOME"]).mkdir()
repo = Path("/work/repo")
repo.mkdir()

def git(*args):
    return subprocess.check_output(["/usr/bin/git", *args], cwd=repo, text=True).strip()

def snapshot():
    return {str(p.relative_to(repo)): ("dir" if p.is_dir() else "link:" + os.readlink(p) if p.is_symlink() else hashlib.sha256(p.read_bytes()).hexdigest()) for p in repo.rglob("*")}

git("init", "-q")
git("config", "user.email", "preview@example.invalid")
git("config", "user.name", "Preview")
(repo / "file").write_text("first\n")
git("add", ".")
git("commit", "-qm", "baseline")
base = git("rev-parse", "HEAD")
for key in ["core.fsmonitor", "diff.external", "diff.sentinel.textconv", "filter.sentinel.clean", "filter.sentinel.smudge"]:
    git("config", key, "touch /work/helper-ran")
(repo / "file").write_text("changed\n")

results = {}
for case in ["regular", "sensitive", "symlink", "promisor"]:
    if case == "sensitive":
        (repo / ".env").write_text("never-read-this")
    if case == "symlink":
        (repo / "file").unlink()
        (repo / "file").symlink_to("/work/external-secret")
        Path("/work/external-secret").write_text("must-not-be-opened")
    if case == "promisor":
        (repo / "file").unlink()
        (repo / "file").write_text("changed\n")
        git("config", "remote.origin.promisor", "true")
        git("config", "remote.origin.url", "https://unreachable.invalid/repository")
    before = snapshot()
    trace = Path(f"/work/{case}.trace")
    p = subprocess.run(["strace", "-f", "-qq", "-s", "8192", "-o", str(trace),
                        "-e", "trace=%file,%network,process,flock,fcntl", "/usr/local/bin/preview_capture_probe", str(repo), base],
                       capture_output=True, text=True, timeout=125)
    assert p.returncode == (0 if case == "regular" else 2), p.stderr
    assert snapshot() == before
    assert not Path("/work/helper-ran").exists()
    text = trace.read_text()
    # Suppressed Git diagnostics/stdio legitimately open the null sink writable.
    write_opens = [line for line in text.splitlines() if re.search(r"\b(?:open|openat|openat2)\(.*\bO_(?:WRONLY|RDWR|CREAT|TRUNC)\b", line) and '"/dev/null",' not in line]
    assert not write_opens, "write open attempted: " + repr(write_opens)
    assert not re.search(r"\b(?:rename\w*|unlink\w*|mkdir\w*|rmdir|link\w*|symlink\w*|chmod\w*|chown\w*|truncate\w*|utime\w*|mknod\w*)\(", text), "filesystem mutation attempted"
    assert not re.search(r"\bflock\(|F_(?:OFD_)?SETLK", text), "lock attempted"
    assert not re.search(r"socket\(AF_INET|connect\(", text), "network attempted"
    executables = re.findall(r'execve\("([^"]+)"', text)
    assert executables and set(executables) <= {"/usr/local/bin/preview_capture_probe", "/usr/bin/git"}, executables
    assert "/work/external-secret\", O_" not in text, "symlink target opened"
    if case == "sensitive":
        assert not re.search(r'open(?:at)?\([^\n]*"(?:[^"\n]*/)?\.env"', text), "sensitive source opened"
        (repo / ".env").unlink()
    results[case] = "passed: immutable snapshots; no filesystem mutation/lock/network/helper syscalls; writable opens only /dev/null"
print(json.dumps({"platform": "Linux", "network": "none", "root_filesystem": "read-only", "scenarios": results}, sort_keys=True))
