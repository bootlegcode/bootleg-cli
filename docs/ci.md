# CI operations

[`ci.yml`](../.github/workflows/ci.yml) runs formatting, lint, locked Rust tests,
and Python harness unit tests. The build needs the `bootleg-core` revision in
[`scripts/acceptance-revisions.json`](../scripts/acceptance-revisions.json).
Update pins alongside compatibility changes; the acceptance harness checks them.

## Build performance

Rust jobs cache crates.io archives and the registry index, keyed by runner and
lockfile content. On a lockfile change, a fallback cache can supply unchanged
crates; Cargo still fetches anything missing and builds with `--locked`. Git
checkouts, Cargo credentials, and `target/` are not cached: build outputs can
contain code from private sibling repositories.

CLI checks and full registration disable incremental compilation and omit debug
info for dev/test builds on disposable runners. Full registration downloads a
pinned, SHA-256-verified nextest binary instead of compiling the tool. Release
build profiles are unchanged.

## Private checkout access

`bootleg-core` is public. All workflows check out its pinned revision with the
standard Actions token; CLI checks (including fork PRs) and releases no longer
require a PAT.

Only full registration still uses `BOOTLEG_REPO_READ_TOKEN`. Give it Contents
read-only access to `bootleg-web` and `bootleg-smuggler` only. Deleting the secret
without replacing it will break full registration, but not CLI checks or releases.
Checkout credentials are not persisted, and full registration never runs PR code.

## CLI releases

[`release.yml`](../.github/workflows/release.yml) runs when a `v*` tag is pushed.
From a clean `main` that matches `origin/main`, run one of these commands
with `just`, Python 3.11+, Cargo, and `vinnie` installed. Cargo needs the sibling
`bootleg-core` checkout.

```sh
just release        # patch (same as just release patch)
just release minor
just release major
```

The recipe uses `Cargo.toml` as the starting version, updates it and `Cargo.lock`,
and commits the bump. Vinnie tags that commit, then Git pushes `main` and the tag
together. You need permission to push both refs to `origin`. If the push fails,
fix the cause and retry the printed Git command instead of bumping again.

The release runs the same CLI checks as branch CI and builds against the same
pinned `bootleg-core` revision. Once all builds pass, it publishes one GitHub
Release with generated notes, binaries, and matching `.sha256` files for Linux
x86-64 (GNU and musl), macOS (Intel and Apple Silicon), and Windows x86-64.
A version like `0.2.0-rc.1` is published as a prerelease. Full registration
is not a release gate. Nothing is published to Homebrew or crates.io.

## Full registration

[`registration.yml`](../.github/workflows/registration.yml) runs manually or
weekly on `main` on a disposable hosted Linux runner. It exercises the full
[acceptance harness](acceptance.md) on exact sibling pins, never against development
services. The Linux relay binds only the owned Docker bridge gateway and forwards
traffic from that subnet to the loopback web service. Direct Chromium runs without
its sandbox only in the disposable Linux GitHub Actions environment.

To reproduce from clean tracked files and pinned sibling checkouts, follow the
[acceptance setup](acceptance.md#setup), then run:

```sh
python3 scripts/ci-registration.py --expect-failure-after-kind
python3 scripts/ci-registration.py
```

The first run must reach kind discovery, hit the specific injected failure, and
verify cleanup. The second requires every scenario to pass and verifies removal
of owned Docker resources and private scratch files. The workflow uploads only
`ci-report.json` and `ci-cleanup-report.json`. Don't upload raw harness output or
use artifact globs that include temporary files. The wrapper captures raw output
privately and discards it; a failure before the harness emits a report may leave
no artifact. Hosted VM teardown covers cases where cleanup can't run, such as
SIGKILL.

For wrapper unit tests without the integration stack:

```sh
python3 scripts/test-ci-registration.py
```
