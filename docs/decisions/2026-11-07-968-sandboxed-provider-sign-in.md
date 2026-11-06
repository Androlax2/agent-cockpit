# A sandboxed provider signs in with the owner's per-provider credential, and its folder trust comes from `projects.trusted`

Status: accepted

Issue #968. Decided by Theo: the sign-in is #858's option B, and the folder trust is written by
Torlek from `projects.trusted` (answer A, relayed by the coordinator).

- **One credential per provider, bound through the proxy.** The owner saves it in
  `<config>/provider-credentials/<provider>/<VARIABLE>`, the file named after the variable the
  adapter declares (`cockpit_provider_api::CredentialBinding`): for Claude
  `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`. It is read by the GitHub tokens' rules
  (`0600` or `0400`, one line); two files for one provider refuse the launch rather than pick
  one. The session proxy adds it on the binding's hosts, the agent holds
  `cockpit_sandbox::PLACEHOLDER` in that variable, and the value reaches only the launcher's
  owner-only credentials file. An unsandboxed session never reads these files.
- **A named header beside `Authorization`.** An API key travels in `x-api-key`, so a binding
  names its header (`CredentialHeader`); the proxy replaces the client's field of that name. Only
  a lower-case `x-` header is accepted, never one that frames or routes the request.
- **The folder trust is written, never carried.** A sandboxed agent's home is the session's own
  and goes with it (#969), so the answer given in one session cannot reach the next. Before the
  agent starts, Torlek records the trust in that home's Claude configuration (created owner-only
  when missing) for a project in `projects.trusted`: its main checkout under
  `sandbox = "required"`, its parallel clones as #702 already did. It reads nothing an agent wrote.
- **Rejected:** mounting or copying the owner's `~/.claude/.credentials.json` or `~/.claude.json`
  into the sandbox (#858): either shows the owner's account to a sandboxed agent.
- **Residual:** a project not in `projects.trusted` asks for the folder trust at each sandboxed
  session; Codex and OpenCode bindings are declared but not exercised with a real harness.

Evidence: `crates/cockpit-daemon/tests/sandbox.rs`
(`the_providers_credential_is_added_on_its_hosts_and_the_agent_holds_a_placeholder`,
`a_trusted_projects_folder_trust_reaches_each_sandboxed_session_of_it`,
`without_a_credential_session_new_and_doctor_name_the_files_to_write`).
