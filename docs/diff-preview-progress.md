# Diff preview implementation progress

## Milestone complete — read-only command and acceptance

`bootleg diff --container-id …` and `bootleg --json diff --container-id …` are
implemented. See the [command guide](diff-preview.md) for supported source scope,
conservative fail-closed refusals, exits 0/1/2 and disclosure/race caveats.
The historical increments below describe development checkpoints, not current
missing features.

Completion commits: `51260a8` (capture/CLI/acceptance), `f0fb89c` and `a94e548`
(pagination fixture corrections), `414c6ca` (special-file/race hardening).
Only CLI changed; core/web/smuggler pins and wire protocols are unchanged.

- `preview_capture.rs` integrates root/configuration/index/history/object gates,
  one union of paths, shared no-follow source bytes, reserved input budgets,
  raw blob comparisons, changed-path attributes and final race revalidation.
- `preview_fs.rs` uses rustix descriptor-relative no-follow traversal, bounded
  regular-file reads, inode/mode/size/mtime/ctime evidence and a bounded directory
  inspection for untracked special files Git omits. Ignored directories are not
  entered; submodules/symlinks/special candidates are refused, not followed.
- The command reuses project/origin/credentials and authorized discovery, with
  no login, credential refresh, bootstrap notices or mutation. Output is buffered,
  deterministic, metadata-only and all-or-nothing. Human errors use stderr;
  JSON errors retain the versioned envelope. No transfer/apply is enabled.
- Actual temporary repositories cover final-worktree precedence, multiple
  baselines, missing/divergent/shallow/replacement cases, ignored/untracked files,
  renames/deletions, binary/mode/attribute/sensitive failures, special files,
  submodule refusal, input limits, descriptor substitution and source/index races.
- Real web/stores/registered-agent acceptance covers clean/different baselines,
  dirty/unknown/offline failures, authorization denial, all exit codes and
  deterministic source snapshots. 201 valid retained/offline DB fixtures force
  selected registered targets onto a later real API page; no server is mocked.
- Linux strace runs with network disabled and read-only root filesystem verify
  regular, sensitive, symlink, promisor and untracked-special scenarios: no
  filesystem mutation, lock, network or helper execution; writable opens only
  to `/dev/null` for suppressed Git stdio. Fixture snapshots remain unchanged.

Final validation: `cargo test --locked` **76 passed / 4 explicitly ignored**;
optional real fzf test separately passed; clippy all targets `-D warnings`,
formatting and whitespace checks passed. `python3 scripts/test-device-auth.py
--kind` passed on implementation `414c6ca`, with cleanup completed; durable
[registration report](diff-preview/registration.json). The report's CLI tracked-edit
flag reflects the concurrently regenerated syscall JSON report, not implementation
edits. `python3 scripts/test-preview-syscalls.py` also passed on that implementation;
[syscall report](diff-preview/syscalls.json). No owned acceptance containers remain;
the pre-existing development cluster was untouched.

Conservative refusal is intentional, not an exact-container-drift claim: linked
worktrees, alternate/promisor stores, all sensitive candidates, symlink/gitlink
candidates and oversized unchanged input are rejected. The implementation does
not promise an atomic filesystem snapshot or secret detection. See the guide for
the full limits. Earlier failed pagination runs were fixture errors (nested
metadata identity and UUIDv4 TypeID validation); their owned resources were cleaned
before corrected, passing full reruns.

## Domain foundation (2026-10-26)

`src/preview.rs` implements pure v1 policies and CLI-owned output types:

- Explicit, sorted, deduplicated target selection; all-or-nothing dirty, unknown,
  offline, missing and inconsistent-duplicate target gates.
- Baseline relationship gate requiring local objects, matching object format,
  valid HEAD and positive ancestry evidence.
- Root-relative UTF-8 path validation, sensitive-path policy and pre-read gate.
- Raw-byte regular-text comparison with add/modify/delete metadata, binary,
  attribute and executable-mode gates; no content-bearing output fields.
- Incremental candidate/input budgets and changed-path/target/output bounds.
- Deterministic, bounded JSON, safe human filename quoting, static errors and
  result exit-code classification (0/1/2).

**No `bootleg diff` command is exposed yet.** This is a domain foundation, not a
completed preview implementation or evidence of adapter read-only behavior.
The existing best-effort `git::inspect` is deliberately not reused.

The pre-read gate fails closed for sensitive candidates unless the adapter can
prove them unchanged without opening worktree contents. It does not authorize
reading sensitive files in order to decide whether they changed. Its boolean
proof parameter must not be inferred from index stat-cache equality.

### Validation

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
git diff --check
```

Results: 43 tests passed, 4 explicitly ignored (fzf and real-server prerequisites);
clippy, formatting and whitespace checks passed. Ten new policy tests cover
selection, baseline evidence, comparisons, redaction, ordering, escaping, and
boundary limits. These are pure policy tests, **not** actual-repository capture,
helper/network/syscall proof or registered-agent acceptance tests. Full pinned
registration acceptance was not rerun for this library-only change. Dependency
pins and wire contracts are unchanged.

### Remaining before the milestone can close

1. Validate the implemented authorized HTTP adapter against the real web/stores
   and registered agents (including authorization, pagination and timeout cases).
   Bounded traversal is implemented; real-server evidence remains outstanding.
2. Integrate the bounded plumbing and typed evidence parsers below into capture;
   validate promisor/shallow/replacement cases and isolated syscall evidence.
   Select a trusted Git executable and map adapter errors at integration.
3. Repository configuration/sparse-checkout gates, object completeness, and
   changed-path attribute checks without filters/textconv. Typed index parsing
   now rejects conflicts, intent-to-add, assume-unchanged and skip-worktree flags;
   these checks still need capture integration.
4. Descriptor-relative no-follow filesystem capture, one source view shared by
   baselines, before/after HEAD/index/enumeration/metadata race detection,
   symlink/type/special-entry rejection and submodule state inspection without
   recursion. Reserve budgets before allocations, including Git output parsing.
5. Clap/config/authorized HTTP integration, deterministic human rendering,
   buffered output and operational error-envelope/exit handling. State the
   reported-build disclaimer and source exclusions; document filename/size
   disclosure and limitations of concurrent-source capture.
6. Actual temporary-repository matrix in `diff-preview-v1.md`, immutable-state
   snapshots and helper/no-fetch/no-lock evidence, then real registered-agent
   discovery/authorization cases and full pinned registration acceptance.

## Application orchestration increment

`src/preview_application.rs` adds narrow synchronous discovery/capture/snapshot
ports (compatible with the existing blocking HTTP stack), an incremental discovery
sink, and all-or-nothing orchestration:

- Validate explicit target IDs before discovery; count all pages/records/body bytes,
  retain only requested metadata, and make limit violations sticky.
- Require complete traversal before target selection; late inconsistent duplicates
  and discovery failure prevent local capture.
- Pass one shared 120s absolute deadline to adapters and check it between operations.
  Blocking adapters must enforce it internally; these checks do not interrupt I/O.
- Capture once for sorted distinct baselines; gate all baseline evidence through
  domain policy before comparisons; compare once per baseline and associate each
  target with its own result. Reject partial success and validate bounded JSON
  before returning a report.

Seven new tests exercise domain-port fakes and sink/deadline boundaries, including
shared-baseline reuse, deterministic output, dirty/unknown/offline targets, late
traversal failures, comparison failures, content exclusion and all three exits.
These are **not** HTTP mocks or actual-repository adapter safety evidence. The
capture port documents, but does not implement, filesystem/Git safety obligations.
`bootleg diff` remains unexposed. Concrete adapters, rendering and full acceptance
remain outstanding; no dependency pins or wire contracts changed.

Validation: `cargo test --locked` (50 passed, 4 explicitly ignored),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo fmt --check`, and
`git diff --check`. Full pinned registration acceptance was not rerun for this
library-only increment.

## HTTP discovery increment

`src/http/preview.rs` implements `AuthorizedDiscovery` as `InstanceDiscovery`,
using the existing origin-scoped authenticated client and an explicit ProjectId:

- Only GET the authorized instances endpoint, with mandatory client headers,
  redirects/retries disabled and no login, refresh or mutation calls.
- Charge pages before requests and cumulative body bytes before growing the
  response buffer; read with a fixed 8 KiB scratch buffer. Each request's total
  timeout is the smaller of 30 seconds and the remaining invocation deadline.
- Decode only preview metadata, skipping labels/runtime/session fields. Bound
  each decoded page to the requested 200 records; charge records before retaining
  selected metadata in the sink. Reject wrong-project records, repeated cursors,
  missing pagination, invalid responses and incomplete traversal.
- Require protocol version 1. Map failures to static preview codes; never expose
  response error bodies, credentials or transport diagnostic strings.

Five unit tests cover cumulative byte accounting, deadline-before-read behavior,
mandatory pagination/duplicate fields, bounded metadata projection, and missing
credentials before requests. No mock API/WS servers were introduced. These tests
are **not** real-server pagination, authorization or blocked-I/O timeout evidence.

Validation: `cargo test --locked` (55 passed, 4 explicitly ignored),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo fmt --check`, and
`git diff --check` passed. Full pinned registration acceptance was not rerun for
this unexposed adapter increment. No dependency pins or wire contracts changed.
Git/process and no-follow filesystem capture, CLI integration, real-repository
safety evidence and registered-agent acceptance still remain. No diff command is
exposed; the milestone stays open.

## Restricted Git history process increment

`src/preview_git.rs` adds a Unix-only history runner, not yet wired to capture:

- Closed command enum for HEAD, object format, full-ID commit existence and
  ancestry. Reject revision expressions/options as IDs and relative executables.
- Clear inherited environment, preserving only HOME/XDG_CONFIG_HOME for user Git
  configuration; explicitly disable optional locks, lazy fetch, replacements,
  prompts, pager, fsmonitor, hooks and transport protocols. No shell invocation,
  stdin, raw diagnostics or arbitrary command arguments.
- Nonblocking Unix socket output capture, fixed scratch buffer, 4 KiB output
  ceiling, 30s operation/shared invocation deadline minimum, and kill/reap cleanup
  on failure. No reader threads, temporary output/index files or new dependencies.
- Three tests cover actual temporary-repository history/missing objects, full
  file-byte/directory-entry before/after snapshots, helper sentinel configuration,
  invalid inputs, output limits, and blocked/streaming/failed process fixtures.

Validation: `cargo test --locked` (58 passed, 4 explicitly ignored),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo fmt --check`, and
`git diff --check` passed. Process fixtures test runner mechanics only; they do not
replace real Git semantics or real-server acceptance. The snapshot test does not
prove absence of transient writes; isolated syscall/network evidence, promisor and
replacement scenarios remain outstanding. Full pinned registration acceptance was
not rerun for this unexposed increment. Other repositories and pins are unchanged.

Still required: bounded tree/index/blob operations, repository/index/attribute and
submodule gates, no-follow shared source capture and race checks, error projection,
CLI/rendering integration, full repository matrix and registered-agent acceptance.
Non-Unix platforms have no adapter yet; integration must reject unsupported ones.
No `bootleg diff` command is exposed; the milestone remains open.

## Bounded source plumbing increment

`src/preview_git.rs` now also supports closed operations for recursive NUL-delimited
leaf trees, NUL-delimited index stages/flags, standard-ignore untracked enumeration,
object sizes and raw blobs. All use the existing restricted process environment,
deadlines and cleanup. No diff/filter/textconv command is used.

- Enumeration checks records incrementally before retaining each output chunk:
  at most 100,000 records, each at most 4,096 path bytes plus a 96-byte plumbing
  header allowance. Empty or unterminated records fail. The typed parser must
  still enforce the exact path-byte limit and validate headers, modes and IDs.
- Raw blob output is capped at 1 MiB, including when size preflight was skipped;
  binary bytes are preserved internally for later domain rejection, not rendered.
- The caller must validate/pass the repository root, enforce the union-of-paths
  and cumulative distinct-input budgets, gate sensitive paths before blob reads,
  and reject unsafe repository/index state. These operations alone do none of
  those checks. Index stage/-v evidence does not yet detect intent-to-add.
- Three new tests exercise actual temporary-repository enumeration and raw reads,
  unusual paths, untracked ignore rules, worktree/blob separation, helper sentinel
  configuration, exact/over-limit binary blobs, incremental record bounds, and
  before/after file-byte and directory-entry snapshots.

Validation: `cargo test --locked` (61 passed, 4 explicitly ignored),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo fmt --check`, and
`git diff --check` passed. No other repositories or dependency pins changed.
Full pinned registration acceptance was not rerun for this unexposed increment.
Snapshots do not prove the absence of transient writes or network attempts.

Still required: typed enumeration/evidence parsing, repository/index/attribute and
submodule gates, no-follow shared capture and race checks, error projection and
CLI integration, remaining real-repository/helper/syscall/network tests, real-server
authorization and full pinned acceptance. No `bootleg diff` command is exposed;
the milestone remains open.

## Typed Git evidence and index gates increment

`src/preview_evidence.rs` adds strict internal tree, index and untracked-path
parsers. They return byte-sorted owned entries, not user-facing diagnostics:

- Validate exact UTF-8 path limits/root-relative safety, modes, object types,
  full nonzero IDs of the expected SHA-1/SHA-256 width, record termination and
  duplicates. Symlinks/gitlinks remain typed entries, not authorized reads.
- Index plumbing now includes `--debug`. Its five metadata lines are framed
  separately from the NUL-terminated pathname, capped at 512 bytes per entry
  before retention. Newlines/tabs in names remain unambiguous.
- Require stage zero and ordinary `H` tags; reject assume-unchanged, skip-worktree,
  intent-to-add, stage/extended/unknown flags and sparse-directory entries. Parse
  the debug format strictly and fail closed for unsupported Git output formats.
  Index stat metadata is not used as proof of worktree equality.
- Six additional tests cover malformed/truncated/duplicate/path evidence,
  incremental index framing bounds, actual SHA-1/SHA-256 repositories, unusual
  filenames, executable/symlink/gitlink entries, and each rejected index state.
  Real index bytes remain unchanged and no persistent index lock is present.

Validation: `cargo test --locked` (67 passed, 4 explicitly ignored),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo fmt --check`, and
`git diff --check` passed. Other repositories and pins are unchanged; full pinned
registration acceptance was not rerun for this unexposed library increment.

Remaining: repository/configuration/sparse gates, attributes, submodule state,
object completeness, aggregate source budgets, sensitive pre-read gates,
descriptor-relative no-follow shared capture and race detection, integration and
rendering, remaining actual-repository matrix and syscall/network evidence,
real-server authorization and full pinned acceptance. The parser accepts bounded
runner output; it does not itself discover or validate a repository or capture
source contents. No `bootleg diff` command is exposed; the TODO remains open.

The v1 design remains the contract; this progress note does not relax its gates.
