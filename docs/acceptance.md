# Registration acceptance checkpoint

## Presence capacity follow-up

Measured shared snapshot locks reduce heartbeat p99 from 1,379.9 ms to 44.4 ms
at 100 agents / 256 SSE viewers / two replicas (300-second matched runs).
No observed false expiry; tested envelope, startup caveats, artifacts and exact
commands: [presence-capacity.md](presence-capacity.md). Full `--kind` acceptance
rerun is green at the pins below; all owned resources were cleaned. Durable report:
`docs/presence-capacity/66fc0c7d260f-registration.json`.

## Presence quota follow-up

Active-session admission now counts retained offline reconnects, while allowing
connected replacement without increasing occupancy. Independent PostgreSQL pools
and Valkey connections race admissions with small limits in the real-store suite.
`just test`: 109 passed; `bash scripts/test-integration.sh --presence`: 4 passed.
Full `python3 scripts/test-device-auth.py --kind` passed with the revisions below;
sanitized report: `.pi/acceptance/bootleg-acceptance-3931a14dfc59/report.json`.
The harness reported successful cleanup. An earlier tool-timeout interrupted run
(`bootleg-acceptance-dabaf4a50814`) was manually cleaned by exact owned resource
prefix before retry; no development services or existing clusters were changed.
These quota tests establish correctness only; subsequent capacity evidence is
linked above.

## Reproduce

Use sibling checkouts `bootleg-{core,cli,smuggler,web}` under one parent. The harness
checks exact compatible dependency revisions in `scripts/acceptance-revisions.json`
before building; it never checks out/reset repositories for you:

| Repository | Revision |
| --- | --- |
| core | `17a784d0f0fe654e88caf226274ef9f792d193fb` |
| web | `4e67ab48fe875ed8725830424e9236991191eae5` |
| smuggler | `7869c5da4015894eb523232e0285c11140d03143` |
| CLI | this capacity-harness commit, built on `cca2f9f` |

Tracked dependency edits require explicit `--allow-dirty-dependencies` and are
recorded in the report. Untracked handoff documents are preserved, not removed.

Prerequisites: Rust/Cargo, **cargo-nextest**, Docker daemon, Python 3 (standard library
only), Node, fzf, and Unix PTYs. Full Docker/kind acceptance currently supports
**macOS Docker Desktop** with `host.docker.internal`; unsupported hosts fail the
explicit request rather than being counted as passing. Install `kind`, `kubectl`,
and web's locked Node dependencies plus Chromium:

```sh
cd bootleg-web
npm ci
npx playwright install chromium
cd ../bootleg-cli
# Full registration acceptance, including isolated kind and Docker:
python3 scripts/test-device-auth.py --kind
```

`--kind` implies `--docker`. Tested with kind v0.33.0; the node image is pinned to
`kindest/node:v1.32.11@sha256:5fc52d52a7b9574015299724bd68f183702956aa4a2116ae75a63cb574b35af8`.
Kind's experimental Docker-network override keeps nodes on this run's owned
network. Its use is an explicit compatibility limitation, not a production deployment
recommendation. The syscall-test image installs strace from Debian repositories;
network access for tool/image acquisition is required. Docker build cache and base
images are retained, not pruned. One free Docker address-pool allocation is needed;
if exhausted, do not delete unrelated networks. Avoid concurrent store suites on
hosts with limited address pools.

Smaller runs explicitly skip prerequisite-dependent scenarios:

```sh
python3 scripts/test-device-auth.py            # skips Docker/syscall proof and kind
python3 scripts/test-device-auth.py --docker   # skips kind
```

All variants build the actual CLI/web/smuggler binaries with `--locked`. They do not
run web's `just rebuild`, which would recreate development services. Stores bind
randomized loopback ports. Accounts and projects exist only in throwaway PostgreSQL
18. Account setup uses actual HTTP registration forms, then narrowly scoped DB
email-verification fixtures; email verification/delivery is not asserted. Device
approval/denial uses Chromium UI. Agent credentials enter only at runtime; agents
never inherit DB credentials or web signing keys. Tokens are absent from arguments,
images and build arguments. Kubernetes uses runtime Secret references; kubeconfig
and secret source files live in the private temporary home.

## Scenario results

`--kind` completed successfully on macOS. **Registration acceptance is green at this
checkpoint**, with the evidence levels below. Capacity/load evidence is reported
separately in [presence-capacity.md](presence-capacity.md). This does not claim a
complete web CI pass, file synchronization or diff preview.

| Scenario | Result / evidence level |
| --- | --- |
| Login approval, pending long poll and denial | PASS — actual CLI + Chromium UI |
| Private credential files / no token in login output | PASS — executable/filesystem assertions |
| Interactive configuration, cancellation, multiple orgs/projects | PASS — actual CLI + fzf PTY |
| CLI project creation and explicit rotation | PASS — executable + real web/stores |
| Expired credentials, logout/revocation and recovery | PASS — CLI + Chromium; expiry set in disposable DB |
| Cross-org isolation | PASS — two real accounts/CLI credentials, disjoint org discovery, bidirectional project discovery denial, unauthorized rotation returns not_found |
| Plain Docker agent in CLI and browser | PASS with --docker — actual built image + real discovery |
| Two kind replicas and two instrumented containers per pod | PASS with --kind — two pods, app/worker per pod; four distinct ContainerIds, Downward API metadata and labels matched to actual pod UIDs |
| Duplicate display labels | PASS — two native agents remain distinct |
| Unknown/dirty baselines | PASS — native metadata in CLI and actual Chromium-rendered baseline cells |
| Network interruption/offline deadline | PASS with --docker — dedicated network disconnect, offline within 45-second lease + 10-second observation allowance |
| Reconnect identity/session | PASS with --docker — same ContainerId and different SmugglerSessionId after network reconnect |
| Process restart identity | PASS — real native smuggler restart creates a new ContainerId |
| Old-session heartbeat/disconnect fencing | PASS — supplemental real-store domain/adapter and actual WebSocket-client tests; **not smuggler executable evidence** |
| Rotation rejects new and existing old-key agents | PASS — CLI rotation, executable agents become offline within 25-second observation bound; new old-key process creates no record after three seconds |
| Deactivation rejects new/existing agents | PASS — disposable DB mutation, actual agents offline within 25 seconds; new inactive-project agent creates no record |
| Control-plane outage preserves application | PASS — kill/restart only owned web process; application file timestamps continue advancing |
| Authorization loss preserves application | PASS — application timestamp continues advancing after rotation/deactivation |
| Disabled mode no agent network I/O | PASS with --docker — strace across actual Linux process tree records zero network syscalls; enabled positive control attempts network I/O; both preserve application exit 23 |
| Browser SSE without manual refresh | PASS — real Chromium DOM changes on actual agent changes |
| Hidden tabs disconnect/reconnect | PASS — native document visibility with focus emulation disabled, CDP observes old SSE request ending and a new request on foreground |
| Interrupted stream marks retained data stale | PASS — actual web process outage; rows retained, stale status, automatic recovery |
| Permission loss clears retained data | PASS — disposable account deactivated during established stream; rows removed/results hidden |
| Agent labels rendered as text | PASS — literal img/onerror label, no injected img/b DOM nodes |

The supplemental fencing suite runs **after** executable/browser scenarios, against
new test databases in the same disposable PostgreSQL container and the same owned
Valkey instance. The native web process is stopped first. The command is:

```sh
cargo run --locked -p bootleg-infrastructure --example test_database -- --presence
```

The harness supplies isolated environment settings; do not run it with development
store URLs. It exercises independent store adapters and real WebSocket connections,
not a canned transport server. It intentionally is not relabeled as executable-agent
acceptance.

## Diagnostics, failures and cleanup

Each run prints an owner-only allowlisted JSON report path:
`.pi/acceptance/bootleg-acceptance-<unique>/report.json`. It records scenario status,
repository revisions, tracked-edit flags, failure class and cleanup status. It never
persists raw exceptions, cookies, keys, UI HTML or secret-bearing DTOs. Not-reached
scenarios are skipped, not passed. `registration_complete` is true only when every
scenario passed and cleanup succeeded. A partial run may exit zero with explicit
skips; automation requiring full registration must check that field or use `--kind`.

Raw web/agent/strace logs and credentials stay inside the temporary home and are
deleted. Failed-run console captures are private until reviewed. Local successful
build/report log: `/tmp/bootleg-acceptance-kind.log`; the sanitized JSON is the durable
handoff artifact.

Cleanup terminates owned children, removes only owned container IDs/volumes, deletes
only the newly named kind cluster using its private kubeconfig, then removes its
network and uniquely tagged images. The temporary home removes kubeconfig, secrets,
Chromium profiles and logs. SIGTERM/SIGINT unwind cleanup; SIGKILL/host failure cannot
be cleaned synchronously. Never prune shared Docker state or delete resources using
current-context. The existing `bootleg` cluster was left untouched and remained
listed after successful runs.

## Findings and limitations

- Cross-org token rotation exposed existing hidden IDs through forbidden responses.
  Web `515974d` fixes this; visible non-manager members still receive forbidden.
  Real-store tests verify hidden/member/deleted/inactive-org outcomes and no key change.
- Full Chromium is launched and attached with Playwright `noDefaults: true`. Default
  focus emulation falsely keeps background pages visible; no visibility property or
  EventSource is mocked. Hidden streams are verified through real CDP network events.
- Project deactivation preserves read visibility. Account eligibility loss is a
  separate browser authorization test, not inferred from project deactivation.
- One development run exhausted Docker's default address pool while another disposable
  store suite ran concurrently. Neither suite pruned unrelated networks. The harness
  no longer reports failed network creation as a failed removal of a nonexistent network.
- `just test` in web: 109 passed, 61 skipped. Full web integration: infrastructure
  43 passed; account/view suite 8 passed, 3 failed on unchanged dashboard markup
  expectations; later security suite not reached. Workspace formatting check also
  reports existing drift. See `../bootleg-web/docs/registration-acceptance.md`.
- Phase 2 quota/concurrent-admission regressions and measured snapshot capacity are
  complete for the explicitly bounded envelope in `presence-capacity.md`. The
  few-agent registration scenarios alone do not establish capacity.
- Coordinated CI and remaining core/smuggler documentation reconciliation are open.
  Diff preview is not implemented; no transfer or remote mutation was added.
