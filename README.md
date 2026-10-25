# Bootleg CLI

Configure a local project and discover its running smuggler agents. Presence means control-plane connectivity, not application readiness. File synchronization is not implemented yet.

## Configure a project

Install [`fzf`](https://github.com/junegunn/fzf) on your PATH for interactive selection, then:

```sh
bootleg config
```

The normal user experience targets Bootleg's hosted API through a built-in default. Most users should never need to know or change the API URL. `BOOTLEG_API_URL` is an advanced override for testing against another server and, potentially, future enterprise/on-prem installations—not a normal setup step.

`bootleg config`:

1. Checks your login. If you are not logged in (or your credential expired/revoked), explains what is happening and starts browser-approved login.
2. Lists the organizations you belong to. Uses the only organization automatically, or opens an **fzf type-ahead fuzzy picker** when there are multiple.
3. Opens a project picker for the selected organization's active projects. Names include IDs so duplicate names remain distinguishable.
4. Saves the API origin, organization ID and project ID to `.bootleg.json`, using the same configuration initialization as `bootleg init`.

Type to filter, Enter to select, Esc/Ctrl-C to cancel. Cancellation, empty selections, and failed requests leave the existing repository configuration unchanged. Login may already have saved a user credential. The picker requires an interactive terminal; `--json` changes only the final result, not the interactive requirement.

For scripts or explicit selection:

```sh
bootleg login
bootleg orgs list
bootleg orgs use org_<id>
bootleg projects list
bootleg projects use project_6m8x91b0349vbrq30fxn997q5s
# Offline equivalent when you already know the IDs:
bootleg init --project-id project_6m8x91b0349vbrq30fxn997q5s --org-id org_<id>

bootleg whoami
bootleg instances list --watch
bootleg logout
```

`orgs list` and `projects list` show readable tables (`*` marks the saved selection); `--json` returns arrays of API records. Empty lists are successful. `orgs use ORG_ID` validates membership, saves the organization, and clears the project selection. `projects list` and `projects use PROJECT_ID` accept `--org-id ORG_ID` to override the saved organization; only active projects in that organization can be selected. Selection changes are saved to `--config PATH` (default `.bootleg.json`) only after validation. These commands require login and never start an interactive picker. `whoami` shows your email, user ID, credential ID, and expiry; `--json` returns the identity record without a token.

There is no standalone `bootleg baseline` command. Instance results retain local Git relationship information when the agent reports a commit. Missing metadata is unknown; matching commits never prove filesystem equality. The CLI does not fetch missing objects.

**Pod names belong to smuggler.** The CLI neither reads a local `POD_NAME` nor stores pod-name settings in repository config. Smuggler discovers/resolves its own runtime label and reports it. Legacy `name_env` repository settings are ignored and omitted on the next save.

## Browser login

`bootleg login` prints a unique approval URL and matching code, then opens your default browser. Compare the code and explicitly authorize the device on the website. An existing website session is reused; otherwise sign in there. The CLI never asks for your password.

Use `bootleg login --no-browser` on remote terminals, and optionally `--device-name "Work laptop"` to customize the device label. Otherwise a unique OS-based name is generated; you can edit it during approval. Browser-opening failure leaves the printable URL available.

The CLI uses bounded long polling (30-second HTTP timeout for the backend's 20-second polls). Credentials expire after 30 days without refresh. Re-run login, or let `bootleg config` start login automatically. Website device revocation takes effect on subsequent requests; password changes do not revoke CLI credentials. If approval succeeds but local storage fails, revoke that device on the website before starting a fresh login.

## Project credentials

```sh
bootleg projects create example --token-file /private/path/project-token
bootleg project token rotate --confirm --token-file /private/path/new-project-token
```

Selecting/configuring an existing project does not recover its key. Creation and rotation deliver the one-time key to a mandatory new owner-only file (0600), reserved before mutation; existing files/symlinks are never overwritten. Keep the file outside Git, provision it using your secret manager, and remove it after delivery. Keys never appear in normal output or `--json`. Rotation invalidates agents' old key.

Mutations never automatically retry. On an uncertain result reconcile server state before trying again; an undelivered key may require an explicitly confirmed rotation. An ambiguous device exchange requires fresh login.

## Configuration and security

The intended API URL precedence is `--api-url` > `BOOTLEG_API_URL` > repository `api_url` > built-in hosted API default. Leave these overrides unset for normal hosted use. For testing against a local server, for example:

```sh
BOOTLEG_API_URL=http://localhost:3000 bootleg --allow-http config
```

**Development status:** the current implementation still requires an explicit API URL or an existing repository `api_url`; the hosted default has not yet been wired in. The production endpoint needs to be confirmed before that implementation change. The override above is a development workaround, not the intended onboarding experience.

Project IDs are typed strings such as `project_6m8x91b0349vbrq30fxn997q5s`, not integers. For existing repository configs with numeric IDs, remove the old `project_id` and run `bootleg config` to select the project again.

Use `--config PATH` for another repository file. Saved settings are only `api_url`, `org_id`, `project_id`. Changing API origin clears inherited selections. URLs must be HTTPS origins without paths, userinfo, queries or fragments. Explicit local development may use `--allow-http` / `BOOTLEG_ALLOW_HTTP=true`. Redirects and automatic HTTP retries are disabled.

User credentials use a **Unix owner-only file fallback**, not an OS keychain: `$HOME/.config/bootleg/credentials` (0700 directory, 0600 files), scoped to canonical API origin. There is no additional encryption at rest. Protect home-directory backups. Non-Unix file storage/delivery fails closed.

CI can supply `BOOTLEG_USER_TOKEN` plus mandatory matching `BOOTLEG_USER_TOKEN_ORIGIN`. This is a user credential, not a project key. If interactive config replaces an expired CI credential through login, unset/update that environment token before subsequent commands. Logout revokes remotely before removing local credentials; 401 counts as already revoked, while network failure never claims success. Remove CI environment values separately.

Human output uses consistent rounded tables/panels on interactive terminals, with cyan informational headings, green success messages, amber warnings, and red errors. Narrow terminals use stacked list records. Redirected output and `TERM=dumb` omit table borders; IDs and URLs are wrapped rather than truncated. Human-output color is enabled by default, including redirected output; use the global `--no-color` flag to disable it. Errors and notices remain on stderr. Clap help/argument validation retain their native formatting, and the interactive picker remains managed by `fzf`.

`--json` emits structured output without styling; watch emits newline-delimited snapshots. Watch refreshes sequentially every three seconds after each completed refresh and emits a stale marker then exits on failure. Announcements use stderr only and are controlled by `--notices=auto|always|never` / `BOOTLEG_NOTICES`. Auto displays them only on interactive stderr outside JSON mode. Errors are never suppressed. Bootstrap is best effort (two seconds), once per network invocation; offline commands do not bootstrap.

## CLI development and tests

These commands compile/test the **CLI executable itself**, not your application. Requires Rust (edition 2024) and sibling `../bootleg-core` at coordinated checkpoint `05144d6e047fc5c88afd58143d9f6771fd262af7`.

```sh
cargo build --locked
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --check
# Optional real fzf adapter test (fzf must be installed):
cargo test --lib fuzzy::tests::real_fzf -- --ignored
```

Selection policies and config completion are directly tested without API mocks. Tests also run real Git, filesystem operations and offline CLI commands. `fzf` is a subprocess adapter; server labels are data, not shell commands. Domain/application policy lives separately from Clap, HTTP and interactive adapters.

Real-server tests are opt-in, using the actual web service/backing stores and real provisioned credentials:

```sh
# Set BOOTLEG_TEST_API_URL, BOOTLEG_TEST_USER_TOKEN,
# BOOTLEG_TEST_EXPIRED_USER_TOKEN, BOOTLEG_TEST_HIDDEN_PROJECT_ID.
# For explicit local HTTP: BOOTLEG_TEST_ALLOW_HTTP=true.
cargo test --test real_server -- --ignored
```

Run the repeatable partial registration acceptance test with Docker, Python 3, `fzf`, Node, and the sibling core/web/smuggler checkouts (see [tested revisions and results](docs/acceptance.md)):

```sh
python3 scripts/test-device-auth.py
```

This builds all three binaries and starts disposable PostgreSQL, Valkey, Mailpit, real web, and native smuggler processes. Chromium approves/denies CLI login using the actual UI. The suite also exercises real `fzf` under a PTY, project creation/rotation, and native agent discovery. Only its disposable account is marked email-verified directly in the database. Containers and temporary credentials are cleaned up on exit. No mock API/WS server substitutes are used. OS browser dispatch is not tested.

This is **partial acceptance**, not the full registration milestone. Docker/kind, browser SSE and additional failure scenarios are explicitly skipped. Diff preview remains gated. See [acceptance results](docs/acceptance.md).
