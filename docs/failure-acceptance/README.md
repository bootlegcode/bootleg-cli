# Mutation ambiguity and harness failure acceptance

Run from the CLI repository with the prerequisites/pins in [acceptance](../acceptance.md):

```sh
python3 scripts/test-acceptance-failures.py
# Individual cases (repeat --case as needed):
python3 scripts/test-acceptance-failures.py --case mutations --case browser-timeout
```

The matrix runs actual binaries and disposable stores, not API mocks. Its
allowlisted summary is `.pi/acceptance/failure-matrix.json`. Reviewed durable
results are in [results.json](results.json); the full successful registration
regression is separately recorded in [registration.json](registration.json).
The failure matrix intentionally does not claim full registration completion.

## Assertions

- **Creation / rotation / device exchange:** a loopback forwarding proxy buffers
  the real web response, then closes the client connection without sending any
  response bytes after a successful mutation. All other responses pass through.
  Web's public origin is the proxy origin, preserving device URL origin checks.
  Actual PostgreSQL counts show exactly one project, one additional rotation audit
  event, and one exchanged grant/credential. No matching request follows the
  dropped response. The actual CLI exits unsuccessfully, removes reserved token
  files, saves no lost login token, and explains reconciliation or device revocation
  plus fresh login. Pending device polls before approval remain permitted.
- **Startup:** invalid bind configuration makes the real web process exit after
  stores/migrations have been provisioned.
- **Browser timeout:** real Chromium loads the approval page with private cookies,
  writes a nonsecret readiness marker, and deliberately stalls. Parent timeout
  must occur after that marker, not simply before browser startup.
- **SIGINT / SIGTERM:** delivered to the harness after login and project creation,
  when owned processes, stores and temporary credentials exist.
- **Partial Docker:** fail after the real agent image and owned network are created,
  before the Docker agent is launched.
- **Partial kind:** fail after creating the real isolated cluster, loading the image,
  and creating a namespace/runtime Secret, before deploying workloads.

Each failure must reach its intended injection point and have the expected failure
class. Cleanup terminates owned process groups (including browser descendants),
removes owned containers/volumes, kind cluster, network and tagged images, and
removes the temporary home before reporting success. Container/network ownership
intent is recorded before creation. The matrix independently checks absence of
run-prefixed Docker resources, preservation of the pre-run unrelated container,
network and image inventory, and an unrelated temporary credential sentinel.
Process-group absence and private-home deletion are asserted by the harness.
Base images and Docker build cache are retained deliberately.

## Privacy and limits

Raw web/browser/proxy/SQL responses are never exported. Temporary raw harness logs
are deleted even on assertion failures; sanitized per-run reports retain failure
class and script/line locations. The matrix exports only explicitly selected
booleans, source report identifiers, revision hashes and tracked-edit flags.
Reports with tracked edits truthfully identify them; they are not pristine-revision
claims. No transfer or remote file mutation functionality is added.

SIGKILL, host failure, or an external tool that kills the process tree without
allowing `finally` to run cannot guarantee cleanup. Recover only the exact
`bootleg-acceptance-<id>` owned resources: use the sanitized report directory/run
prefix, inspect matching containers/networks/image tags and kind cluster, and
remove only those resources. Use the matching private kubeconfig if available;
never change the developer's current context, prune Docker globally, or remove
unrelated networks to recover address-pool capacity. Remove the corresponding
`bootleg-cli-e2e-*` private directory only after establishing its ownership.
See [general cleanup guidance](../acceptance.md#diagnostics-failures-and-cleanup).
