# CLI workstream handoff

Scope: `bootleg-cli` only. No sibling repository/shared contract edits. Core checkpoint: `05144d6e047fc5c88afd58143d9f6771fd262af7` (path dependency; shared fixtures read-only).

## Current surface

- login/logout/whoami, **orgs list/use**, projects list/create/use, confirmed token rotation, init, interactive **config**, and instance watch.
- `bootleg config` checks login eligibility and starts browser login with an explanation for missing/expired/revoked credentials. It uses the sole organization automatically, otherwise an fzf fuzzy picker, then always prompts for an active project. Both config/init use `RepositoryConfig::initialize` and save only API/org/project identity after successful selection.
- No standalone `baseline` command. Git relationship inspection remains attached to instance results.
- No pod/name-env settings in saved CLI config. Legacy name-env config is discarded on save. Smuggler owns pod identity.
- No application build step or deployment generators.
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

`src/main.rs`, `src/config.rs`, `src/application.rs`, `src/lib.rs`, new `src/selection.rs`, new `src/fuzzy.rs`, `tests/offline_cli.rs`, `README.md`, `HANDOFF.md`.

Earlier CLI implementation also added/changed `Cargo.toml`, `Cargo.lock`, `src/domain.rs`, `src/git.rs`, `src/http.rs`, `src/storage.rs`, `tests/real_server.rs`.

## Integration blockers / coordinator actions

1. `python3 scripts/test-device-auth.py` now builds CLI/web/smuggler, provisions disposable real stores, and exercises Chromium approval/denial, real PTY fzf configuration, expired credentials and logout. See `docs/acceptance.md` for exact revisions, prerequisites and scenario results.
2. CLI project create/rotation and native smuggler discovery, duplicate labels, baseline metadata, restart identity and rotation invalidation passed. Real Chromium now verifies SSE updates, text-only labels, stale data/recovery on actual web outage, and eligibility-loss clearing. Project deactivation and continued application file updates are checked. `--docker` (macOS Docker Desktop) passed real image discovery and network disconnect/reconnect with identity preservation and session replacement. These are executable acceptance checks, not protocol-client substitutes. Owner-only sanitized scenario reports remain in `.pi/acceptance/`.
3. Full registration acceptance is still incomplete: kind, cross-org isolation, stale-session callbacks, disabled-network proof, browser hidden tabs, and ambiguous mutations remain unproven here. Quota/capacity work and diff preview have not started; the diff gate remains closed.
4. Existing coordinator clarification: the frozen contract requires agent plaintext-development opt-in but does not specify its flag/env spelling. Deployment examples intentionally do not invent a shared setting.

Secure file fallback remains Unix-only; no OS keychain adapter claimed. Human results are structured text; watch uses NDJSON in JSON mode and emits a stale marker on refresh failure before exiting. Core remains a coordinated sibling dependency, not a published release.
