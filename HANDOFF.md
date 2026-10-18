# CLI workstream handoff

Scope: `bootleg-cli` only. No sibling repository/shared contract edits. Core checkpoint: `5d37918fb8aef80f9b5a8cc443e7241e0af1d419` (path dependency; shared fixtures read-only).

## Current surface

- login/logout/whoami, **orgs list/use**, projects list/create/use, confirmed token rotation, init, interactive **config**, optional deployment examples, and instance watch.
- `bootleg config` checks login eligibility and starts browser login with an explanation for missing/expired/revoked credentials. It uses the sole organization automatically, otherwise an fzf fuzzy picker, then always prompts for an active project. Both config/init use `RepositoryConfig::initialize` and save only API/org/project identity after successful selection.
- No standalone `baseline` command. Git relationship inspection remains attached to instance results.
- No pod/name-env settings in saved CLI config. Legacy name-env config is discarded on save. Smuggler owns pod identity. Optional deployment examples may reference agent-side runtime environment variables; generators never read laptop pod variables.
- No application build step. Optional Docker/Compose/Kubernetes generators only print deployment fragments for containers already running smuggler, without applying or building anything.
- Origin-scoped owner-only Unix credential fallback, CI origin binding, reserved owner-only project-token delivery. No automatic mutation retries or secret-bearing DTO output.

## Boundaries and tests

- Domain policies: device polling, HTTPS origins, notices, Git relationship decisions.
- Selection use cases: automatic sole organization, explicit multi-org/project choice, active/org filtering, empty/cancel/invalid selections, IDs distinguishing duplicate names. Direct prompt-port tests, no API/WS mocks.
- fzf process adapter: server labels treated as data, terminal controls removed, selection mapped by index rather than display name. User-supplied fzf default command/options are not inherited. Requires installed fzf and an interactive terminal; preflight runs before login/network side effects.
- HTTP/storage/Git/Clap remain edge adapters. Real filesystem/Git/offline executable tests plus an opt-in real fzf filtering test.

Validation:
- `cargo test --locked`: 21 passed; 3 real-server tests and 1 fzf adapter test explicitly ignored by default.
- `cargo test --lib fuzzy::tests::real_fzf -- --ignored`: real installed fzf adapter test run separately.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `git diff --check`.

## Latest change files

`src/main.rs`, `src/config.rs`, `src/application.rs`, `src/lib.rs`, new `src/selection.rs`, new `src/fuzzy.rs`, `src/deployment.rs`, `tests/offline_cli.rs`, `README.md`, `HANDOFF.md`.

Earlier CLI implementation also added/changed `Cargo.toml`, `Cargo.lock`, `src/domain.rs`, `src/git.rs`, `src/http.rs`, `src/storage.rs`, `tests/real_server.rs`.

## Integration blockers / coordinator actions

1. No actual web deployment or provisioned eligible/expired/cross-org credentials were exercised. Opt-in tests are provided, not claimed as passing integration tests.
2. Interactive browser login + fzf configuration with actual organization/project responses still needs live end-to-end validation. The real fzf filter test is not a claim that the whole interactive workflow was exercised.
3. Real project mutations/logout, registered smuggler snapshots, Docker/kind acceptance, and ambiguous mutation failure after server commit/before response still require coordinated testing using real components and controlled fault injection. No canned API server substitutes.
4. Existing coordinator clarification: the frozen contract requires agent plaintext-development opt-in but does not specify its flag/env spelling. Deployment examples intentionally do not invent a shared setting.

Secure file fallback remains Unix-only; no OS keychain adapter claimed. Human results are structured text; watch uses NDJSON in JSON mode and emits a stale marker on refresh failure before exiting. Core remains a coordinated sibling dependency, not a published release.
