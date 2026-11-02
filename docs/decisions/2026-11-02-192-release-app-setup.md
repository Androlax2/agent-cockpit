# Bootstrap the Release App through GitHub's interactive manifest flow

Issue #192. Theo requested creating the nonexistent Release App and retaining the existing
workflow variable/secret names. The worker chose the following implementation boundaries.

- **Private App, one repository.** Request contents/write and pull_requests/write only,
  with GitHub's implicit metadata/read. Verify owner, permissions and the installation's
  complete repository list before configuring `Androlax2/agent-cockpit`. No webhook events.
- **Browser confirmation remains human-owned.** The official manifest flow requires
  registration confirmation; installing on selected repositories is another browser step.
  A loopback callback exchanges the one-time code without printing it. Host execution is
  reviewed through `ask-to-run`; readiness precedes `ask-to-guide`.
- **Recovery fails closed.** Store the returned PEM in owner-only state outside Git and
  upload via GitHub CLI stdin. Refuse mismatched existing credentials, uncertain secret
  writes and interrupted conversions rather than creating another App or rotating a key.
  Completed repeats do not write credentials. Secret metadata cannot prove key equality.
- **No new dependency or release behavior.** Use Python's standard library, existing `gh`
  and OpenSSL. The release-plz history-preserving proposal remains unapproved and separate.
- **Rejected:** assuming fully unattended registration, adding a PAT or broader App scope,
  silently replacing existing secrets, or claiming fixture tests prove live setup.

Implementation and isolated evidence: [setup script](../../tools/setup_release_app.py),
[tests](../../tools/tests/test_setup_release_app.py), and
[operator instructions](../RELEASING.md#release-app-setup).
