# Presence capacity: measured checkpoint

## Supported/tested envelope

On the host below: **100 connected agents in one project, 256 authenticated SSE
viewers evenly distributed across two web replicas**, with the default 10 database
connections per replica. The final 300-second observation acknowledged 2,000
heartbeats with zero reported errors, missing agents or observed false offline
states (45-second leases). Viewers establish first; agents arrive at 10 attempts/s.
One replica with 100 agents / 128 viewers and a small 10-agent / 8-viewer case also
passed. This is a local tested envelope, not a production SLA, maximum capacity,
or a claim about 1,000 active / 10,000 retained records, many projects, long-lived
soaks, WAN latency, reconnect storms, or simultaneous cold start. Re-measure on
the deployment hardware. The SSE limit remains 128 streams **per web process**.

## Repeat

Use acceptance prerequisites in [acceptance.md](acceptance.md), including the
locked web Node dependencies. No additional Python or Node dependency is needed;
the driver uses the WebSocket client bundled with pinned Playwright 1.64.0.
From `bootleg-cli`:

```sh
python3 scripts/test-device-auth.py --presence-load 10,8,1,90
python3 scripts/test-device-auth.py --presence-load 100,128,1,120
python3 scripts/test-device-auth.py --presence-load 100,256,2,300
python3 scripts/test-device-auth.py --kind
```

The command builds actual debug binaries with Cargo `--locked`, uses real HTTP
account creation and Chromium device approval, then starts additional native web
processes sharing **only this run's disposable PostgreSQL and Valkey**. WebSocket
clients send real registration/15-second heartbeat frames; HTTP clients consume
the actual cookie-authorized SSE endpoint. These are protocol-level load clients,
not smuggler executable or browser-rendering evidence. Full `--kind` acceptance
separately verifies actual smuggler processes and Chromium rendering.

Only a unique resource prefix is deleted. Development services, existing Docker
networks and the existing `bootleg` kind cluster are preserved. Every recorded
run below reports completed cleanup; no `bootleg-acceptance-*` containers remained.
Raw credentials/logs live in the temporary home and are deleted. The allowlisted
`load.json` and `report.json` are private in `.pi/acceptance/<run>/`; reviewed copies
are committed in [presence-capacity/](presence-capacity/). Load runs intentionally
leave `registration_complete=false`; they do not masquerade as full acceptance.

## Environment and revisions

- Apple M4 Max, 16 host CPUs, 128 GiB RAM; macOS 15.7.9 arm64.
- Docker Desktop server 29.8.2, 16 CPUs / 50,498,068,480 bytes VM memory.
- Rust 1.98.1 (`48a229cea`, 2026-09-01), Node v26.12.1, Python 3.15.0;
  Playwright 1.64.0. Native web and load clients share this host. Other development
  services remained running; this is not a dedicated benchmark machine.
- PostgreSQL 18.6, Valkey 8.1.10; loopback published ports to Linux containers.
- PostgreSQL image: `postgres:18-alpine`, digest
  `sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873`.
- Valkey image: `valkey/valkey:8-alpine`, digest
  `sha256:081c2f5cb575efc901aa80ff9cdbd1ec6a301682fd35e1ebb4b0990a4a4a8507`.
- Fixture mail: `axllent/mailpit:latest`, digest
  `sha256:b1f1be18af530d939a11ee8820b379e0c88eeec204d904bfad68862adced3a5a`.
  Harness tags can move; use these digests when reconstructing this environment.
- Core `17a784d0f0fe654e88caf226274ef9f792d193fb`; smuggler
  `7869c5da4015894eb523232e0285c11140d03143` unchanged.
- Web baseline `bc257c4d96cf5a605fc3c82124f9fe889289c5a4`; improved code committed as
  `4e67ab48fe875ed8725830424e9236991191eae5` and deliberately pinned in acceptance.
- Measurements used CLI `cca2f9fbafa7786a46a7fba383f47790312e427d` plus this harness.
  After measurements used tracked web edits and explicit
  `--allow-dirty-dependencies`, recorded in reports. The final baseline had the
  added helper/test present but called the original exclusive `lock` in
  `instances`; final after called `read_lock`. To reconstruct baseline, change
  that one call to `lock` and explicitly allow the dirty pinned dependency. Never
  reset another checkout or silently modify pins.

## Final matched before/after (100 agents / 256 viewers / 2 replicas)

Both commands used `--allow-dirty-dependencies --presence-load 100,256,2,300`
and staged viewer readiness. Registration takes about ten seconds before the
300-second observation; sampled PostgreSQL data includes startup/registration.
Times below are milliseconds, nearest-rank percentiles. Full counts and numerical
precision are in JSON. No failures in either run.

| Metric | Before p50 / p95 / p99 / max | After p50 / p95 / p99 / max |
| --- | --- | --- |
| Registration, 100 samples | 238.3 / 1660.5 / 1818.6 / 1916.7 | 10.4 / 20.0 / 21.4 / 27.3 |
| Heartbeat acknowledgment, 2,000 samples | 926.7 / 1233.9 / 1379.9 / 1583.2 | 5.8 / 14.8 / 44.4 / 137.5 |
| Snapshot HTTP including body | 1741.3 / 1944.3 / 2024.7 / 2150.2 | 8.9 / 25.4 / 40.9 / 71.2 |
| SSE first snapshot, 256 streams | 575.8 / 732.2 / 747.4 / 751.7 | 469.6 / 498.8 / 509.2 / 511.5 |
| SSE event gap | 1762.3 / 3194.2 / 4458.2 / 4974.0 | 1000.4 / 4999.6 / 5006.6 / 5092.5 |

Snapshot requests run sequentially, once per replica per observation loop, with
one second between loops (136 before / 586 after requests). This is closed-loop
latency, not an independent fixed-rate request generator. Registration is timed
from WebSocket construction through registered acknowledgment; heartbeat is
send-to-ack using the monotonic clock. Failed requests are counted separately,
not silently excluded from successful latency distributions.

SSE event gaps are **not delivery latency**: unchanged snapshots are suppressed,
and agents update every 15 seconds. The `sse_age_ms` artifact is receive wall time
minus snapshot server time, which starts after acquiring the project lock. It
excludes pre-lock/pool waiting and is subject to host/VM clock skew (negative
values in two exploratory runs); do not use it as end-to-end freshness. SSE first
snapshot latency includes startup authorization/queueing. No browser paint or
heartbeat-to-browser propagation latency is claimed.

### Pool pressure / advisory contention

The sampler queries `pg_stat_activity` from the disposable container approximately
every 0.5 seconds **plus docker-exec/query overhead**. It excludes its own backend.
Columns below show p50 / p95 / p99 / max across 577 before / 565 after samples.

| Metric | Before | After |
| --- | --- | --- |
| Database connections | 20 / 20 / 20 / 20 | 20 / 20 / 20 / 20 |
| Active backends | 18 / 20 / 20 / 20 | 0 / 5 / 20 / 20 |
| Advisory-lock waiters | 18 / 19 / 19 / 19 | 0 / 0 / 6 / 13 |
| Oldest waiting query age, ms | 115.5 / 136.9 / 149.8 / 185.9 | 0 / 0 / 7.1 / 18.9 |

These are sampled pressure proxies, not sqlx pool-acquisition timings or complete
lock-wait duration histograms. Query age includes pre-wait execution; short waits
may be missed. All 20 connections can still be briefly active after the change.

## Decision and safety

Keep snapshot transactions authoritative but acquire a **shared project advisory
lock** instead of an exclusive one. Mutations still take the exclusive lock on
the identical key. Readers overlap but cannot interleave with cross-store writes;
locks remain held through Valkey observation. Authorization is still rechecked per
stream each second, row locks still order eligibility changes, stale session
callbacks remain fenced, and store errors fail closed. No process-local registry,
snapshot cache, new TTL or fan-out mechanism was needed.

The final runs show heartbeat p99 down ~31x and snapshot HTTP p99 down ~50x. The
small-case noise and machine variability do not support universal speedup claims.

## Run inventory and failures

IDs refer to `presence-capacity/<id>-load.json` plus `<id>-report.json`.
The exploratory driver started agents while viewers were opening. Only the final
matched pair waits for all viewers' initial snapshots first.

| Run suffix | Lock | Agents / viewers / replicas | Seconds | Heartbeat p99 ms | Outcome |
| --- | --- | --- | --- | --- | --- |
| `0925d71fd143` | exclusive | 10 / 8 / 1 | 90 | 10.1 | pass |
| `f88a660918ba` | exclusive | 100 / 128 / 1 | 120 | 612.1 | pass |
| `e98d54f18c9b` | exclusive | 100 / 256 / 2 | 120 | 1427.7 | pass |
| `44fafa2694fe` | shared | 10 / 8 / 1 | 90 | 16.4 | pass |
| `1a7daaaaa01b` | shared | 100 / 128 / 1 | 120 | 258.6 | pass |
| `3dc9031929b4` | shared | 100 / 256 / 2 | 120 | 31.2 | pass |
| `b9782e6bc631` | shared | 100 / 256 / 2 | 300 | 309.0 | **fail**, 7 registrations rejected |
| `a61150ca49b7` | exclusive, staged | 100 / 256 / 2 | 300 | 1379.9 | pass |
| `0d0a063439b5` | shared, staged | 100 / 256 / 2 | 300 | 44.4 | pass |

The failed combined-startup run did not record HTTP rejection status, so its cause
is unproven (pending-upgrade quota pressure is plausible). The driver now records
status-only rejection counters and explicitly stages viewer readiness. No retry
conceals a failure. This narrows the supported envelope; it does not prove that
cold-start bursts are fixed. Early driver bring-up also rejected non-UUIDv4 test
IDs and misread the flat wire DTO as CLI display DTO; both were corrected before
the inventory runs. Their private reports (`3816ca6d90ef`, `b14e3010c4e0`) record
successful cleanup and are not server capacity evidence.

## Regression and acceptance checkpoint

- Web `just test`: 109 passed, 62 skipped.
- `bash scripts/test-integration.sh --presence`: 4 passed, including a new real-store
  assertion that overlapping readers complete, heartbeat waits for shared readers,
  and renewal succeeds after release. Existing fencing/quota/authorization cases pass.
- Full `python3 scripts/test-device-auth.py --kind`: **GREEN**, all scenarios passed,
  cleanup completed. Durable report: `presence-capacity/66fc0c7d260f-registration.json`.
  Includes actual Docker/kind agents, Chromium, revocation, reconnection and
  supplemental real-store/WebSocket regressions against the newly pinned web commit.
- Existing unrelated full-dashboard test and workspace formatting failures remain
  tracked separately. No core/smuggler changes or pins were needed.
