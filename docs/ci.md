# CI operations

Repository workflows run formatting, lint and locked Rust tests on Ubuntu 24.04
with Rust 1.98.1. Core regenerates schemas/fixtures and rejects tracked or untracked
drift. Smuggler additionally builds its real Docker image and tests Linux PID 1
TERM/KILL behavior. Web separates domain tests, direct JavaScript tests, disposable
real-store contracts and actual Chromium tests. Existing web Clippy warnings are
visible but not fatal (TODO-02dc902a); lint errors are fatal. No workflow runs
`just rebuild`: that command touches development services.

CLI checks read the core SHA from `scripts/acceptance-revisions.json`; web and
smuggler carry an explicit compatible core SHA in their workflows. Update these
pins deliberately alongside compatibility changes. GitHub Actions are commit
pinned. Cargo/npm use lockfiles. Container base tags and transitive tool installers
are not claimed to be fully reproducible supply-chain pins.

## Private checkout authentication

CLI, web and smuggler require Actions secret `BOOTLEG_REPO_READ_TOKEN`, a
fine-grained token with Contents read-only access to `bootlegcode/bootleg-core`.
**Full registration additionally needs read access to bootleg-web and
bootleg-smuggler on the token installed in CLI.** No write permission is needed.
The default repository-scoped Actions token cannot fetch private siblings.
Deploy keys are disabled by repository policy. Do not install a broad personal
admin token as a workaround. Fork PRs do not receive this secret and cannot run
these private-dependency checks without a separate trusted review workflow.

## Full registration runner

`registration.yml` runs manually or weekly, only on `main`, on a GitHub-hosted
Ubuntu 24.04 disposable VM. It does not need a self-hosted Mac or enable variable.
The Linux path has been implemented but is **not yet validated end-to-end**:
the first run stopped at sibling authentication before any acceptance scenarios.
Do not treat the workflow's existence as registration evidence.

The job provisions Rust 1.98.1, Node 24.11.1, Python 3, fzf, kind v0.33.0 and
Chromium from web's npm lockfile; Docker and kubectl are supplied by the runner.
Kind's node image is digest-pinned by the harness. Its dedicated Docker network
uses kind's experimental network override.

Native web and stores remain bound to loopback. On Linux, a single-process TCP
relay binds **only the newly owned bridge gateway**, accepts peers only from that
bridge subnet and forwards to the fixed loopback web port. Docker/kind agents use
that gateway origin. There is no wildcard listener, host-network container or
shared cluster. macOS local runs retain Docker Desktop host routing. The relay
is tracked with the other native children and terminated during cleanup.

Sibling checkouts use exact 40-character SHAs from the CLI manifest, never default
branches. Checkout credentials are not persisted. The CLI revision is the workflow
event's immutable commit. All sibling tracked-file cleanliness checks remain
enabled; the wrapper also checks CLI cleanliness. No PR event triggers full
registration. GitHub destroys the hosted VM after the job, including on failures;
no developer resources should ever be attached to that VM.

The job first injects a deliberate exception after real kind discovery, when
stores, native processes, Chromium, Docker agents, syscall containers, images and
kind all exist. The wrapper requires that exact failure class and successful kind
evidence, completed cleanup, no remaining owned containers/networks/tagged images,
and an empty private temporary directory. This is cleanup evidence, **not** a
successful registration run. It then runs the full success path and requires exit
zero, `registration_complete: true`, all scenarios passed, no tracked edits and
verified cleanup. Partial zero-exit runs cannot pass this gate. SIGINT/SIGTERM are
forwarded for harness cleanup; SIGKILL/host failure relies on hosted VM teardown.

Only `ci-report.json` and `ci-cleanup-report.json` are uploaded, normalized to
scenario statuses, revision SHAs, edit flags, cleanup/completion booleans, and
allowlisted script names/line numbers for failures (no exception text). Raw harness
output is captured in a private temporary file and deleted, not streamed to
Actions. Never add wildcards for `.pi`, logs, profiles, traces, kubeconfigs, or
temporary homes to uploads. Missing reports fail the gate (early prerequisite/build
failures need not produce one). Repository test output is not uploaded as an artifact.

Local wrapper validation: `python3 scripts/test-ci-registration.py`.
From clean, pinned sibling checkouts with the required Docker tools:

```sh
python3 scripts/ci-registration.py --expect-failure-after-kind
python3 scripts/ci-registration.py
```

## Validation checkpoints

All four repository workflows passed on GitHub Actions after read-token provisioning:
core run `40585576923`, CLI `40585892183`, smuggler `40585905144`, web `40585896686`.
This includes web domain/JS/real-store/Chromium and smuggler Linux/Docker checks.

Full Linux registration run `41320768751` failed fetching the pinned web revision
with HTTP 403. Core checkout succeeded; smuggler checkout and all scenarios were
not reached. Expand the CLI token's selected repositories as above and rerun.
Full success/failure-cleanup evidence remains required before closing the CI TODO.
