# Partial registration acceptance checkpoint

## Reproduce

Sibling checkout layout is required: `bootleg-{core,cli,smuggler,web}` under one parent.
Tested dependency revisions (not floating branch recommendations):

| Repository | Revision |
| --- | --- |
| core | `05144d6e047fc5c88afd58143d9f6771fd262af7` |
| web | `068d0a680736fb95791aa1e7262ac427fd3a6536` |
| smuggler | `7869c5da4015894eb523232e0285c11140d03143` |
| CLI baseline | `d560658fd56b5cac10ca9ad86d05032932084383` plus this acceptance commit |

Prerequisites: Rust/Cargo, Docker daemon, Python 3 (standard library only), Node,
`fzf`, macOS or Unix PTY support. Install web's locked Node dependencies and Chromium:

```sh
cd bootleg-web
npm ci
npx playwright install chromium
cd ../bootleg-cli
python3 scripts/test-device-auth.py
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
| Authorization loss preserves supervised application | PASS — smuggler supervisor remains running |
| Cross-org isolation | SKIP — not implemented in this harness |
| Plain Docker discovery in CLI/browser | SKIP — native agents only |
| Kind replicas / two instrumented containers per pod | SKIP — not implemented; no cluster resources created |
| Network interruption / reconnect identity and session replacement | SKIP — not implemented |
| Old-session heartbeat/disconnect fencing | SKIP — no executable acceptance here |
| Project deactivation | SKIP — not implemented |
| Control-plane outage / disabled zero-network mode | SKIP — not implemented here |
| Browser SSE, hidden tabs, stale data, permission loss, text-only labels | SKIP — browser tests here cover login only |

The harness exits nonzero on assertion failure; zero means **only the implemented
subset passed**, not full registration acceptance. The final console report always
lists the missing groups. Earlier development attempts exposed a changed CLI login
heading, fzf's asynchronous filter timing, and a helper-name collision; these were
fixed before the successful run.

## Diagnostics and cleanup

Console output is the concise report. Successful run build/report log for this
checkpoint: `/tmp/bootleg-acceptance-run.log` (local ephemeral artifact, no credentials).
Raw web/agent logs and credentials are deliberately kept inside the temporary home
and deleted on exit rather than persisted as purportedly sanitized diagnostics.
The suite does not yet provide durable sanitized failure artifacts; assertions may
include UI/API output, so treat failed-run console captures as private until reviewed.

A `finally` block terminates children (kill fallback), removes only owned container
IDs and their anonymous volumes, and the temporary-directory context removes
credentials/logs. SIGTERM is converted into normal exception cleanup; SIGINT also
unwinds. SIGKILL/host failure cannot be cleaned synchronously: inspect only the
printed/named `bootleg-acceptance-*` resources, never prune shared Docker state.
Cleanup errors cause failure. No kind cluster is created or selected.

## Remaining gate

Full registration acceptance is **not green**. Presence quota edge cases, real-store
concurrent admission, snapshot contention measurement, supported capacity numbers,
coordinated CI pinning, and remaining documentation reconciliation are still open.
No quota behavior, authoritative state, remote mutation, file transfer or diff
preview is changed by this checkpoint.
