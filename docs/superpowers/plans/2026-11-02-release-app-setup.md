# Release App setup implementation plan

> **For agentic workers:** Use executing-plans for inline implementation, then one fresh independent full-delta review.

**Goal:** Prepare and carry out the user-requested #192 App registration/install/configuration setup through host approval.

**Architecture:** A Python standard-library script serves GitHub's manifest form on loopback and receives its state-bound callback. It retains a private recovery record outside the checkout, verifies App/install permissions with an OpenSSL-signed JWT, then uses `gh` to configure the repository variable and secret without printing credentials.

**Tech stack:** Python 3.11+, existing GitHub CLI and OpenSSL; no added dependency.

**Spec:** Theo's setup request in this session and issue #192. The release-plz history proposal is separate and unapproved.

## Constraints

- Fixed repository: Androlax2/agent-cockpit; private App owned by Androlax2.
- App permissions: contents/write and pull_requests/write, plus GitHub's implicit metadata/read; no events or active webhook.
- Installation must select only agent-cockpit; verify this before configuration.
- Set RELEASE_APP_CLIENT_ID to the returned client ID, and RELEASE_APP_PRIVATE_KEY via secret stdin.
- Never print keys, tokens, conversion codes or raw error bodies; never save credentials in Git.
- Refuse mismatched existing configuration and uncertain partial writes; repeated successful runs do not rotate credentials.
- Host execution/configuration requires ask-to-run. Ask browser readiness before ask-to-guide.

## Implementation and verification

1. Create `tools/setup_release_app.py` and `tools/tests/test_setup_release_app.py`.
   Write failing tests for the real command entry point and fixture-driven registration/configuration: exact manifest permissions, callback state validation, private recovery files, wrong scope refusal, secret stdin, reruns and interrupted writes. Implement registration and configuration without changing release workflows.
2. Test App API transport without live GitHub: fixtures for App metadata, installation, repository listing and token revocation; a temporary loopback callback and fake gh/OpenSSL commands. Keep credential sentinels out of stdout/stderr/argv.
3. Document browser steps, recovery limits and official sources in `docs/RELEASING.md`; add a dated per-issue decision and coverage-map evidence.
4. Run the focused unittest file, release regression tests, Ruff and changed-document checks. Obtain an independent fresh full-delta review; fix findings before host execution.
5. Commit and open one Part-of-#192 PR. Ask readiness and submit reviewed host commands through ask-to-run, then ask-to-guide for browser confirmation. Verify actual scope and configuration; report live steps that remain incomplete.

## Review focus

- Wrong callback state or replay never converts a manifest.
- Recovery files are private and symlinks refused; conversion interruption does not create another App.
- All-repository or multiple-repository installation never reaches secret writes.
- Existing or externally changed credentials are never silently overwritten.
- Subprocess/API failures do not disclose private responses, and installation tokens are revoked.
