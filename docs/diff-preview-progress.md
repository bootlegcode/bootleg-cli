# Diff preview implementation progress

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

1. Narrow application discovery/source ports and one-capture, distinct-baseline
   orchestration. Freeze preview metadata only after complete bounded discovery;
   enforce 100 pages / 20,000 records / 16 MiB response bytes.
2. Hardened Git process adapter: command/environment allowlist, optional-lock,
   lazy-fetch, replacement, prompt, pager, fsmonitor and helper suppression;
   bounded streaming output, per-operation 30s and invocation 120s deadlines.
3. Actual Git object/history and index/tree enumeration, including unmerged,
   intent-to-add, sparse, assume-unchanged and skip-worktree rejection; raw blob
   reads and changed-path attribute checks without filters/textconv.
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

The v1 design remains the contract; this progress note does not relax its gates.
