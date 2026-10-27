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

## Private core checkout authentication

CLI, web and smuggler require Actions secret `BOOTLEG_REPO_READ_TOKEN`: a
fine-grained token restricted to `bootlegcode/bootleg-core`, Contents read-only.
The default repository-scoped Actions token cannot fetch this private sibling.
Deploy keys are disabled by repository policy. Do not install a broad personal
admin token as a workaround. Fork PRs do not receive this secret and cannot run
these private-dependency checks without a separate trusted review workflow.

## Full registration runner

`registration.yml` runs manually or weekly, only on `main`, and only when repository
variable `REGISTRATION_RUNNER_ENABLED=true`. Otherwise the job is **skipped**, not
registration evidence. Configure environment `registration-acceptance` with approval
and branch restrictions. Do not enable until the runner is provisioned and validated.

Required label: `bootleg-registration-ephemeral` on a **single-use macOS runner with
Docker Desktop**. Hosted Linux/macOS runners are not substitutes: full harness host
routing is macOS Docker Desktop-specific. Provision Rust 1.98.1 with cargo-nextest,
Python 3, Node/npm, fzf, Docker Compose, kind v0.33.0 and compatible kubectl. Docker
must have a free network allocation. Chromium is installed from web's npm lockfile.
The kind node image is digest-pinned by the harness. The dedicated Docker network
uses kind's experimental network override.

Use an empty disposable workspace and no development resources or credentials.
Sibling fetches use exact 40-character SHAs from the CLI manifest, never default
branches. Private repositories require runner-side read-only Git authentication;
never embed tokens in checkout URLs or logs. Do not expose this runner to PRs.
After every job, destroy its VM, including Docker state and workspace, even on
cancellation/host failure. This external lifecycle is a prerequisite, not something
a YAML label implements. Do not use a long-lived developer Mac.

The CLI revision is the workflow event's immutable commit. All sibling tracked-file
cleanliness checks remain enabled; the wrapper also checks CLI cleanliness. Full
acceptance uses `--kind` and requires exit zero, `registration_complete: true`, all
scenarios passed, no tracked edits and completed cleanup. Partial zero-exit runs
cannot pass this gate. SIGINT/SIGTERM are forwarded for harness cleanup; SIGKILL or
host failure requires the external ephemeral runner teardown.

Only `ci-report.json` is uploaded, normalized to scenario statuses, revision SHAs,
edit flags and cleanup/completion booleans. Raw harness output is captured in a
private temporary file and deleted, not streamed to Actions. Never add wildcards
for `.pi`, logs, profiles, traces, kubeconfigs, or temporary homes to uploads.
Missing reports fail the gate (early prerequisite/build failures need not produce
one). Repository test output is not uploaded as an artifact.

Local wrapper validation: `python3 scripts/test-ci-registration.py`.
Run the full wrapper only from clean, pinned sibling checkouts on the provisioned
runner: `python3 scripts/ci-registration.py`.

## Implementation validation

Local core/CLI/smuggler all-target tests, all four formatting checks, and three
report-gate tests passed. Web `just test` exceeded a 180-second command deadline
after compilation; no real-store/browser suite was reached in that invocation.
No full registration or GitHub-hosted workflow execution is claimed by this change.
Runner provisioning, first hosted checks and success/failure teardown validation
remain deployment gates before treating this TODO as complete.
