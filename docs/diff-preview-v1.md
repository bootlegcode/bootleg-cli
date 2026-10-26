# Read-only diff preview — design v1

Status: implemented for the conservative Unix source scope described in
[the command guide](diff-preview.md); validation evidence is linked there.
This document remains the v1 implementation contract.
Version: 1. Owned by `bootleg-cli`; no core wire, web, or smuggler changes.
Registration and measured capacity remain the gate in [acceptance.md](acceptance.md).
The shared `bootleg-core/docs/contract-v1.md` takes precedence for discovery DTOs.

## Meaning and command

```sh
bootleg diff --container-id container_<id> [--container-id container_<other>]
bootleg --json diff --container-id container_<id>
```

Reuse existing origin, project configuration, origin-scoped user credentials and
mandatory client headers. Require at least one canonical ContainerId; deduplicate
and sort IDs. No label selection, implicit all-target selection, interactive login,
watch, path filters, baseline override, patch export or apply option in v1.
Repository is the Git worktree containing the invocation directory, always compared
from its root; `--config` selects configuration, not a different source root.

Always state: **Preview relative to the reported build commit; container filesystem
contents are not verified.** Presence is connectivity, not application readiness.
A clean result means equality only within the declared local source scope. Generated
image files, mounts, later syncs and runtime modifications remain unknown.

Fetch the existing authorized paginated instances endpoint, not agent sockets.
Consume a complete bounded discovery traversal before planning; absent IDs fail as
`target_unavailable` without guessing whether they exist elsewhere. Selected targets
must be connected in their returned snapshot; offline targets fail. Pages are live
reads, not an atomic snapshot. Reject duplicate selected records with inconsistent
metadata. Freeze the selected metadata for this invocation; make no promise that
presence remains current. Do not print display labels or runtime metadata.

## Baseline gate and multiple targets

For **each** target require a full validated build commit and `build_dirty=false`.
Missing commit or null dirty flag is unknown; true is dirty. Require a local commit
object and all required trees/blobs, matching repository object format, and a valid
local HEAD. Accept only baseline equal to HEAD or provably an ancestor of HEAD.
Reject descendant, divergent and indeterminate relationships, including insufficient
shallow history. Detached HEAD is allowed; unborn HEAD is not. Do not substitute
HEAD for missing build metadata or infer cleanliness from commit equality.

Compute one comparison per distinct accepted baseline against one captured local
source view. Targets with differing baselines therefore can have different changes.
Associate every target with its own baseline/result; never collapse them into a
single misleading patch. Any invalid target or unsupported change fails the whole
invocation; no partial successful preview. Unchanged unsupported entries need not
fail unless they prevent complete source enumeration (for example sparse checkout).

## Local source scope

Compare baseline tree bytes/modes with the final working-tree bytes/modes, including
committed changes after the baseline plus staged and unstaged changes. The index
helps identify paths, not the desired file contents: a staged edit reverted in the
worktree is compared using the worktree. Enumerate the union of baseline, HEAD and
index paths plus nonignored untracked files; detect deletions even for baseline-only
paths. Reject unmerged index entries, intent-to-add entries, sparse checkout and
assume-unchanged/skip-worktree entries rather than trusting status shortcuts.

Git's standard ignore rules apply **only to untracked paths** (`.gitignore`, repository
exclude and configured global excludes). Tracked paths never disappear due to ignore
rules. Ignored untracked paths and Git administrative data are outside scope; every
output declares this exclusion. No `.bootlegignore` or ad hoc extension allowlist.
Ignored files are not traversed/read merely to report them. Repository configuration
is not implicitly ignored: if tracked/nonignored, it follows the same rules below.

| Change / entry | v1 behavior |
| --- | --- |
| Regular UTF-8 text file add/modify/delete | Supported, metadata summary only |
| Nonignored untracked regular text | Included as add if absent in baseline; otherwise compare normally |
| Rename/copy | No similarity detection; report independent delete/add (or ordinary modifications at existing paths) |
| Binary (NUL byte or invalid UTF-8 in either compared side) | Reject changed entry; never emit bytes |
| Symlink, special file, type change | Reject changed entry; never follow symlinks |
| Gitlink/submodule | Reject changed gitlink or dirty/untracked submodule state; never recurse, initialize or fetch |
| Executable mode | Preserve/report 100644 or 100755; reject mode-only or accompanying mode changes; additions/deletions may carry either mode |
| CRLF / missing final newline | Compare raw bytes; no normalization or line-count claims |
| Spaces, tabs, newline, leading dash, Unicode paths | Supported using byte-safe enumeration and escaped rendering |
| Non-UTF-8 path | Reject safely without printing raw bytes |

Use NUL-delimited Git plumbing, literal paths and argument arrays, never shell string
interpolation. Compare raw Git blobs and no-follow worktree reads, not Git diff drivers,
textconv, clean/smudge filters or LFS hydration. LFS pointer text is only pointer text;
changed files with `filter`, `working-tree-encoding`, or `diff` attributes set to a
value/true are rejected as `unsupported_attributes` to avoid misleading comparisons.
No content hashes, line excerpts, hunks or raw Git diagnostics in output.

## Sensitive data and read-only guarantees

Metadata-only output is intentional: v1 does not offer a content disclosure override.
Filenames and sizes can themselves be sensitive; document this before users publish
reports. Defense in depth: for a changed path, reject any ASCII-case-insensitive
component `.env`, `.env.*`, `.ssh`, `.aws`, `.gnupg`, `credentials`, or `secrets`, or
basename `id_rsa`, `id_ed25519`, or suffix `.pem`, `.key`, `.p12`, `.pfx`. This is not
secret detection: arbitrary source names may contain secrets. Report only the generic
`sensitive_path` code and count, never the matching path, contents or blob hash.
Check this policy before opening changed worktree contents. Never read credential
stores as source via symlinks, or serialize API/token DTOs into preview output.

Only existing bootstrap/discovery GETs are allowed remotely. No uploads, WS commands,
remote file reads/writes, signals, restart, transfer, sync lock or lease acquisition.
Locally do not change index, refs, objects, config, credentials or source files; no
checkout, stash, refresh, optional Git locks, temporary index or patch files. Disable
optional Git locks, lazy fetch, prompts, external diff/textconv, pager and filesystem
monitor hooks. Disable replacement objects; do not execute repository-configured
helpers. Validate Git adapter commands/environment explicitly, rather than reusing
`git status` and assuming it has no writes. No remote fetch even for promisor objects.
OS access-time updates and server-side request/auth bookkeeping are not sync writes.

Read each candidate safely and bound reads before allocation. Check HEAD/index and
source enumeration/metadata before and after capture; fail `source_changed` on detected
changes. Never follow a path component swapped to a symlink: use descriptor-relative
no-follow traversal or reject unsupported platforms. This is not an atomic filesystem
snapshot; undetected concurrent edits remain possible. Advise users to stop local
writers. All baselines use the same captured bytes, not sequential rereads.

## Bounds and deterministic output

Fixed v1 limits (fail, never truncate or silently skip): 32 distinct requested targets;
100 discovery pages / 20,000 records / 16 MiB cumulative response bytes; 100,000
candidate paths; 4,096 UTF-8 bytes per path; 1,000 changed paths per baseline;
1 MiB per compared blob/worktree file; 16 MiB total distinct compared input bytes;
4 MiB final serialized output. Count bytes before decoding, deduplicate input objects
only by reliable identity, and enforce limits while streaming/enumerating. Bound each
Git operation to 30 seconds and the invocation to 120 seconds. Oversized unchanged
files need not be loaded if equality is established without reading their contents;
otherwise fail the bound rather than trusting stat-cache equality. No CLI overrides.

Sort targets by canonical ID, changes by raw UTF-8 path bytes, diagnostics by code
then target ID. No timestamps, random identifiers, absolute local paths, labels or
session IDs in output. Same captured inputs produce identical JSON bytes; human
output follows existing styling/`--no-color` policy with the same ordering. Escape
all control characters, backslashes and bidi controls in human filenames (never raw
terminal controls); JSON uses valid string escaping, including bidi controls.

The CLI-owned JSON projection has `schema_version: 1`, `status: ok|error`,
`comparison: reported_build_commit`, `filesystem_verified: false`,
`scope: tracked_and_nonignored_untracked`, `ignored_untracked: excluded`,
`targets: []` and `errors: []`. Success targets contain only `container_id`,
`build_commit`, `relationship: equal|ancestor`, and `changes`. Each change has
`path`, `kind: add|modify|delete`, nullable `old_bytes`/`new_bytes`, and nullable
`old_mode`/`new_mode` (six-digit Git mode strings). Null denotes an absent side.
Error results have empty `targets`; each error contains a stable `code`, nullable
`container_id` and safe static `message`, with no source path or subprocess text.
No direct serialization of discovery or credential-bearing DTOs. Buffer and validate
the bounded result before writing stdout; broken output is an operational failure.

Exit codes: **0** complete preview with no changes for any target; **1** complete
preview with changes for at least one target; **2** usage, auth, discovery, baseline,
unsupported-case, bounds, source-race or I/O failure. JSON operational failures use
the envelope above; Clap parse/help behavior remains native and may precede JSON.
Human failures go to stderr, with no successful preview on stdout. Notices stay on
stderr under existing policy. Stable initial failure codes: `target_unavailable`,
`baseline_unknown`, `baseline_dirty`, `missing_object`, `unsafe_history`,
`unsupported_repository`, `unsupported_entry`, `unsupported_attributes`,
`sensitive_path`, `limit_exceeded`, `source_changed`, `discovery_failed`, `git_failed`,
`io_failed`. Extend codes additively; output-shape or comparison changes require a
new design/schema version.

## Implementation and acceptance handoff

Keep baseline/source safety and preview policies in domain code; an application
use case coordinates narrow discovery/Git/source ports. Clap, HTTP, filesystem,
Git process and rendering are adapters. Existing discovery's best-effort baseline
inspection is not sufficient for this fail-closed gate. No API/WS mock servers.

Use actual temporary Git repositories for equal/ancestor/descendant/diverged,
missing/promisor objects, shallow history, detached/unborn HEAD, mixed baselines,
unknown/dirty flags, staged-versus-worktree precedence, deletion/rename, untracked
and ignored paths, binary, modes, symlinks, submodules, attributes, conflicts,
unusual filenames, sensitive names, every limit boundary and detected source races.
Golden JSON tests assert ordering/escaping, no content leakage and all exit codes.
Verify HEAD/index/object/config/source bytes and directory entries unchanged; use
Git helper sentinels and isolated network/syscall evidence to prove no helper,
fetch, lock or mutation occurs. Direct application tests may use domain-port fakes;
authorization/discovery integration must use the real web/stores and registered
smuggler, including multiple baselines, offline/unknown targets and denied access.

Run CLI tests, clippy, formatting and diff checks; preserve pinned full registration
acceptance. Document actual results separately from this design. No transfer/apply
protocol, lock implementation, restart behavior or exact-container-drift claim is
unblocked by completing this preview milestone.
