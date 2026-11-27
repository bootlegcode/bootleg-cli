# CI operations

[`ci.yml`](../.github/workflows/ci.yml) runs formatting, lint, locked Rust tests,
and Python harness unit tests. The build needs the `bootleg-core` revision in
[`scripts/acceptance-revisions.json`](../scripts/acceptance-revisions.json).
Update pins alongside compatibility changes; the acceptance harness checks them.

## Private checkout access

The current workflows need read access to private sibling repositories. For
trusted jobs, `BOOTLEG_REPO_READ_TOKEN` should have Contents read-only access to
`bootleg-core` (also `bootleg-web` and `bootleg-smuggler` for full registration).
Don't use an admin token. Checkout credentials are not persisted. Fork PRs don't
receive the secret and cannot run these checks. Keep credentialed jobs in trusted
private CI before making this repo public; don't grant secrets to fork PR code.

## CLI releases

[`release.yml`](../.github/workflows/release.yml) runs when a `v*` tag is pushed.
Tag a commit containing the workflow and the matching `Cargo.toml` version (update
`Cargo.lock` alongside any version bump). For example, for version `0.1.0`:

```sh
git tag -a v0.1.0 -m 'Release v0.1.0'
git push origin v0.1.0
```

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
