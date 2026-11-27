# Developer acceptance tests

## Setup

Put `bootleg-cli`, `bootleg-core`, `bootleg-web`, and `bootleg-smuggler` under
one parent directory. The sibling repos are private. Check them out at the
revisions in [`scripts/acceptance-revisions.json`](../scripts/acceptance-revisions.json)
from the CLI revision you want to test. The harness checks the pins but never
changes your checkouts. Use `--allow-dirty-dependencies` only to test deliberate
tracked edits at the pinned revisions; the report records them.

Install Rust/Cargo, cargo-nextest, Docker, Python 3, Node, and `fzf` on a host
with Unix PTYs. For `--kind`, also install kind and kubectl. The Docker and kind
paths support Linux and macOS Docker Desktop. Allow network access for downloading
dependencies and images and leave capacity for a dedicated Docker network.

From `bootleg-cli`, install the browser dependencies from the locked web checkout:

```sh
(cd ../bootleg-web && npm ci && npx --no-install playwright install chromium)
```

On a fresh Linux host, install Chromium's system dependencies too (Playwright's
`install --with-deps chromium`). See [CI operations](ci.md) for the hosted setup.

## Run registration

```sh
python3 scripts/test-device-auth.py            # native scenarios only
python3 scripts/test-device-auth.py --docker   # adds Docker and Linux syscall checks
python3 scripts/test-device-auth.py --kind     # full run; includes --docker
```

The harness builds the actual CLI, web service, and smuggler with `--locked` and
uses disposable PostgreSQL, Valkey, Mailpit, and Chromium. It covers approval and
denial, interactive configuration with a real PTY, project creation and rotation,
authorization and revocation, agent lifecycle, browser SSE, and diff preview.
It marks disposable accounts email-verified in the database: this is not a test
of email delivery or OS browser dispatch. Supplemental session-fencing tests use
real stores and a WebSocket client, not the smuggler executable.

## Run targeted checks

```sh
# Committed response loss plus startup, browser, signal and partial provisioning failures:
python3 scripts/test-acceptance-failures.py
# Or select cases (repeat --case):
python3 scripts/test-acceptance-failures.py --case mutations --case browser-timeout

# Local diff capture under strace in a network-disabled, read-only Linux container:
python3 scripts/test-preview-syscalls.py

# Presence load: agents, viewers, web replicas, seconds
python3 scripts/test-device-auth.py --presence-load 100,256,2,300
```

The mutation tests drop successful responses from the real server and check that
the CLI does not retry creation, rotation, or device exchange, and does not leave
behind undelivered credentials. The failure matrix checks that owned resources
are removed and unrelated resources survive. The capture trace checks for file
mutation, locks, helper execution, and network activity in the local capture
adapter, not in the full CLI's HTTP discovery path.

The load driver uses protocol clients against real web replicas and shared
disposable stores. It is not browser-rendering or executable-agent evidence.
Viewers establish their streams before agents arrive; measure on deployment
hardware, and don't infer cold-start resilience or end-to-end delivery latency
from this test.

## Diagnostics, failures and cleanup

Registration prints a private, allowlisted report path under
`.pi/acceptance/bootleg-acceptance-<id>/report.json`. Load runs also write
`load.json`; the failure matrix writes `.pi/acceptance/failure-matrix.json`.
The capture syscall check writes `.pi/acceptance/preview-syscalls.json`.
Review reports before sharing. Do not publish raw logs, profiles, kubeconfigs,
or credentials. A partial run can exit zero with skipped scenarios; automation
requiring full coverage must require `registration_complete: true` or use the
[CI wrapper](ci.md#full-registration).

The harness cleans up its children, private temporary home, and owned Docker and
kind resources, including on SIGINT/SIGTERM. Docker build cache and base images
are retained. SIGKILL or host failure can leave resources behind. Match the exact
run prefix in the report to its containers, network, tagged images, kind cluster,
and private temporary directory before deleting anything. Use the run's private
kubeconfig if available; don't change your current Kubernetes context, prune
shared Docker state, or delete unrelated networks. If address pools are exhausted,
avoid running disposable suites concurrently.
