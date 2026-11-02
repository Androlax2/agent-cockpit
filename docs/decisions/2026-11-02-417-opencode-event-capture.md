# Prove OpenCode's native plugin with an isolated local model

Issue #417. Decided by the worker within Theo's approved conformance scope; Theo explicitly
approved the official OpenCode 1.19.5 archive download and extraction under `.outgoing/`.

- **Use the pinned executable.** Installed OpenCode 2.1.4 cannot prove 1.19.5. The approved
  archive and executable hashes, observed Bun 1.4.4 runtime and captured native records
  are in the [fixture provenance](../../crates/cockpit-opencode/tests/fixtures/opencode-1.19.5/README.md).
- **Use a local model, fresh homes and the real launch/plugin/bridge.** The fixture supplies
  model responses, a brief and a fake Cockpit endpoint. OpenCode itself emits all events;
  the recorder forwards their bytes to the built bridge before recording its reports.
  The exact release bundles the compatible-provider SDK. Its `Npm.install` returns early
  for a non-writable directory, so the temporary config directory is read-only and tests
  assert no package files appeared. No account credentials, installed packages or new
  dependencies are needed.
- **Reject the permission through the owner boundary.** A harmless printf command remains
  pending until the authenticated fixture owner rejects it; an unauthenticated reply gets
  HTTP 401. The bridge emits no permission answer. A separate held model turn is aborted;
  its report has no completion or failure outcome. This measures the API boundary, not a
  terminal screenshot or a signed-in provider account.
- **Keep evidence scopes explicit.** Native fields consumed by the adapter match the
  schema-derived examples, so no production change is needed. Captured records replay
  through the bridge in CI. Unobserved forms (questions, child sessions, retry and API
  errors) remain schema-derived and are not promoted to captured evidence.
- **Track the exposed persistence bug separately.** Native completion is followed by a
  busy status before idle. The adapter calls that a new prompt, clearing the session's
  stored outcome. #516 retains this evidence for a provider/session-boundary regression;
  #417 proves capture, not persistent completion. #51 must preserve that distinction.
- **Rejected:** running the installed newer binary as proof of the pin, synthesizing
  native bus events, accepting a command to finish the probe, or consuming signed-in
  account usage when a local model proves this issue's plugin/event behavior.
