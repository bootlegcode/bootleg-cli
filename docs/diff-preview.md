# Read-only diff preview

```sh
bootleg instances list
bootleg diff --container-id container_<id>
bootleg --json diff --container-id container_<id> --container-id container_<other>
```

Use an existing login and selected project (`bootleg config` or `bootleg init`).
`diff` never logs in, refreshes credentials, changes configuration or retries a
request. It performs only authorized discovery GETs; it does not contact agents.
Unlike other network commands it does not fetch bootstrap announcements.
`--config PATH` selects configuration, not a source directory. Source is the entire
Git worktree containing the invocation directory. Explicit IDs are mandatory,
deduplicated and sorted; every selected target must be connected in the complete
bounded discovery traversal and report a full commit with `build_dirty=false`.

**Preview relative to the reported build commit; container filesystem contents are
not verified.** Generated image content, mounts, runtime changes and subsequent
syncs remain unknown. Presence means connectivity, not readiness.

The preview compares each baseline with one captured final working-tree view,
including committed, staged, unstaged, deleted and nonignored untracked paths.
The index supplies paths, not desired content. Git's standard ignore rules apply
only to untracked files. Renames appear as independent additions/deletions. No
patches, hunks, hashes or content excerpts are emitted. Filenames, byte sizes and
modes are metadata that may themselves be sensitive: review reports before sharing.
Human filenames are quoted and terminal controls escaped; JSON is CLI-owned,
versioned and deterministically ordered. Human preview output is deliberately
plain (including under `--no-color`). Human failures use stderr; JSON failures use
the same bounded envelope on stdout. Broken output returns an operational failure.

| Exit | Meaning |
| --- | --- |
| 0 | Complete preview, no changes for any selected target |
| 1 | Complete preview, at least one changed target |
| 2 | Invalid usage, authorization/discovery, baseline, unsupported source, limit, race or I/O failure |

Clap help and argument parsing can return before a JSON report is created. Every
operational error is all-or-nothing: no partial successful targets or raw
subprocess/server diagnostics are emitted.

## JSON output

`--json` emits `schema_version: 1`, `status` (`ok` or `error`),
`comparison: "reported_build_commit"`, `filesystem_verified: false`,
`scope: "tracked_and_nonignored_untracked"`, `ignored_untracked: "excluded"`,
and `targets` and `errors` arrays. On success, each target gives its
`container_id`, `build_commit`, `relationship` (`equal` or `ancestor`), and
`changes` sorted by path. Each change gives `path`, `kind` (`add`, `modify`, or
`delete`), `old_bytes`, `new_bytes`, `old_mode`, and `new_mode`. Modes are Git
mode strings (`100644` or `100755`); an absent side has null size and mode.
On failure, `targets` is empty and `errors` contains diagnostics with a `code`,
nullable `container_id`, and static `message`.

## Safety and conservative refusals

The tested platforms are macOS and Linux, using the trusted `/usr/bin/git` path.
Other non-Unix platforms fail closed. Git runs with a restricted environment,
optional locks/lazy fetch/replacements/helpers/prompts/transports disabled, bounded
output and operation deadlines. Source reads walk descriptor-relative components
with `NOFOLLOW`, reject nonregular entries, reserve budgets before allocation and
check file/directory identity and metadata before/after reading. A bounded,
no-follow directory-entry inspection also rejects nonignored special files that
Git intentionally omits from untracked listings; ignored directories are not
entered. This inspection has a 100,000-entry work ceiling. HEAD, index,
configuration, source enumeration, attributes and source metadata are rechecked
before returning results. This is **not an atomic snapshot**: stop local writers;
undetected concurrent edits remain possible. No checkout, stash, refresh, fetch,
temporary index, transfer, restart or sync lock is performed. OS access-time updates
and server request/auth bookkeeping are not sync writes.

The v1 implementation deliberately refuses cases it cannot prove safe:

- Dirty/unknown/offline targets; missing local objects; descendant/divergent or
  unprovable shallow history. No baseline fallback or automatic remote fetch.
- Sparse checkout/index, unmerged stages, intent-to-add, assume-unchanged,
  skip-worktree and unknown index flags. Unsupported Git debug formats fail closed.
- Linked worktrees, separate/symlinked Git directories, alternate object stores,
  promisor/partial-clone configuration and enabled worktree configuration.
- All symlink, gitlink/submodule and special-file candidates, even unchanged ones.
  No submodule traversal, initialization, remote inspection or credential-store
  reads through symlinks. Regular/executable additions and deletions are supported;
  executable-mode changes and changed binary/non-UTF-8 content are rejected.
- Every sensitive-path candidate, including unchanged candidates. Matching is
  ASCII-case-insensitive: components named `.env` or starting with `.env.`,
  `.ssh`, `.aws`, `.gnupg`, `credentials`, or `secrets`; basenames `id_rsa` or
  `id_ed25519`; and filenames ending in `.pem`, `.key`, `.p12`, or `.pfx`.
  Matching paths are not printed. This is a filename policy, not secret detection.
- Changed paths with active `filter`, `working-tree-encoding` or `diff` attributes.
  Raw bytes are compared without filters, textconv or newline normalization.

The fixed limits are:
32 targets; 100 discovery pages/20,000 records/16 MiB response bytes; 100,000 union
paths; 4,096 bytes per path; 1,000 changes per baseline; 1 MiB per file/blob;
16 MiB distinct compared input bytes (source and baseline both count); 4 MiB
serialized output; 30 seconds per Git/HTTP operation within a shared 120-second
preview deadline. There are no override flags. Oversized unchanged files are
conservatively rejected rather than trusting index stat metadata.

## Developer checks

See [acceptance tests](acceptance.md#run-targeted-checks) for the isolated local
capture syscall check and the full-stack registration harness that exercises
preview against registered agents.
