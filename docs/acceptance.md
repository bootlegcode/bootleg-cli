# Partial registration acceptance checkpoint

## Reproduce

Sibling checkout layout is required: `bootleg-{core,cli,smuggler,web}` under one parent.
Tested dependency revisions (not floating branch recommendations):

| Repository | Revision |
| --- | --- |
| core | `05144d6e047fc5c88afd58143d9f6771fd262af7` |
| web | `068d0a680736fb95791aa1e7262ac427fd3a6536` |
| smuggler | `7869c5da4015894eb523232e0285c11140d03143` |
| CLI baseline | `e0bffa6` plus this Docker/browser acceptance commit |

Prerequisites: Rust/Cargo, Docker daemon, Python 3 (standard library only), Node,
`fzf`, macOS or Unix PTY support. The optional `--docker` scenario currently requires
macOS Docker Desktop's `host.docker.internal` routing; it rejects unsupported hosts
rather than claiming success. Install web's locked Node dependencies and Chromium:

```sh
cd bootleg-web
npm ci
npx playwright install chromium
cd ../bootleg-cli
python3 scripts/test-device-auth.py
# On macOS Docker Desktop, include isolated Docker network lifecycle:
python3 scripts/test-device-auth.py --docker
```

The last command builds all native executables with `--locked`. It deliberately
does not run web's `just rebuild`, which recreates development services. No existing
project, service or cluster is touched. Stores bind randomized loopback ports;
Docker names carry a unique run prefix. Account/project data lives only in the
throwaway PostgreSQL container. Credentials enter agents via runtime environment
and Chromium via stdin, never arguments, images or build arguments. Account setup
uses real HTTP forms, then a narrowly scoped disposable DB email-verification
fixture; email delivery/verification itself is not an acceptance assertion.

## Results

Executed successfully on macOS with real local sockets, Chromium, CLI, native
smuggler and web. No canned HTTP/WebSocket servers or protocol-client replacements.

| Scenario | Result / evidence level |
| --- | --- |
| CLI login approved through UI, pending long poll, denial | PASS — CLI + Chromium |
| Credential permissions and no token in login output | PASS — executable/filesystem |
| Interactive config, cancellation, multiple orgs/projects | PASS — CLI + real fzf PTY |
| CLI project creation and explicit rotation | PASS — executable + real web/stores |
| Expired credential recovery and logout/revocation | PASS — CLI + Chromium; expiry set in disposable DB |
| Duplicate labels remain distinct | PASS — two native agents + CLI |
| Unknown and dirty baseline projection | PASS — native agents + CLI DTO assertions, not browser rendering |
| Process restart changes ContainerId | PASS — native smuggler restart + CLI |
| Rotation removes connected presence by next checks | PASS — native agents + CLI; 65-second observation bound |
| Old key rejects new agent | PASS — native executable; no new retained record after three seconds |
| Authorization loss preserves supervised application | PASS — application file timestamp continues advancing |
| Cross-org isolation | SKIP — not implemented in this harness |
| Plain Docker discovery in CLI/browser | PASS with `--docker` — actual image, CLI and Chromium |
| Kind replicas / two instrumented containers per pod | SKIP — not implemented; no cluster resources created |
| Network interruption / reconnect identity and session replacement | PASS with `--docker` — disconnect/reconnect dedicated Docker network; same ContainerId, new SmugglerSessionId; offline within 45-second lease plus 10-second observation allowance |
| Old-session heartbeat/disconnect fencing | SKIP — no executable acceptance here |
| Project deactivation | PASS — disposable DB change; agents offline by next check, new agent rejected, application continues |
| Control-plane outage | PASS — kill/restart owned web process; native applications keep updating files |
| Disabled zero-network mode | SKIP — not implemented here |
| Browser SSE and text-only labels | PASS — actual Chromium, live snapshots before/after agent changes, no refresh or request mocks |
| Browser stale data and reconnect | PASS — actual server outage closes sockets; retained rows marked stale, automatic recovery |
| Browser permission loss | PASS — disposable account deactivated while stream active; rows cleared and results hidden |
| Browser hidden tabs | SKIP — not implemented |

The harness exits nonzero on assertion failure; zero means **only the implemented
subset passed**, not full registration acceptance. The final console report always
lists the missing groups. Earlier development attempts exposed a changed CLI login
heading, fzf's asynchronous filter timing, and a helper-name collision; these were
fixed before the successful run. Subsequent testing replaced a fixed fzf filter sleep
with observation of the filtered result count while draining the PTY. A Chromium
offline toggle did not reliably interrupt an established SSE stream, so outage
checks now terminate the actual disposable web process instead. Project deactivation
preserves member read visibility; browser eligibility loss is tested separately by
deactivating the disposable account.

## Diagnostics and cleanup

Console output is the concise report. The current successful Docker run's build/report
log is `/tmp/bootleg-acceptance-docker.log` (local ephemeral artifact). Every run also
writes `.pi/acceptance/bootleg-acceptance-<unique>/report.json`: an owner-only,
allowlisted scenario report with repository revisions, failure class (never raw error
text), cleanup status and explicit skips. It remains after temporary resources are
removed. Not-reached scenarios are not passed. Reports always state
`registration_complete: false` while mandatory scenarios remain unimplemented.
Raw web/agent logs and credentials stay inside the temporary home and are deleted
on exit. Failed-run console captures may include UI/API/DB diagnostics and must be
treated as private; only the allowlisted JSON is a sanitized diagnostic artifact.

A `finally` block terminates children (kill fallback), removes only owned container
IDs and their anonymous volumes, and the temporary-directory context removes
credentials/logs. SIGTERM is converted into normal exception cleanup; SIGINT also
unwinds. SIGKILL/host failure cannot be cleaned synchronously: inspect only the
printed/named `bootleg-acceptance-*` resources, never prune shared Docker state.
Cleanup errors cause failure. The optional Docker scenario builds from a temporary
source allowlist (no sibling secrets or `.git` in its build context), injects environment
values at runtime, and removes only its uniquely tagged image and named network.
Docker build cache/base images remain available, not pruned. No kind cluster is
created or selected.

## Remaining gate

Full registration acceptance is **not green**. Presence quota edge cases, real-store
concurrent admission, snapshot contention measurement, supported capacity numbers,
coordinated CI pinning, and remaining documentation reconciliation are still open.
No quota behavior, authoritative state, remote mutation, file transfer or diff
preview is changed by this checkpoint.
