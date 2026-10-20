# Native Codex CLI in the cockpit

Parent task: `1d3fd318`. Theo explicitly requested native Codex CLI as well as Claude Code CLI and OpenCode. This prerequisite is independently testable; it does not implement collaboration or weaken the approved team boundaries.

## Outcome

Native Codex appears as an agent choice and has the existing cockpit lifecycle: launch, view/focus, parallel/background task launch, resume, restart, restore and close. Its verified state, ask-to-run/ask-to-guide attention and todo attribution behave like other supported agents. Missing native features are refused by capability, not sent as another CLI's shortcuts.

Keep Codex's native terminal UI, approvals and authentication. Do not replace it with a custom chat UI or run it unsandboxed. Model and effort controls use verified Codex-supported options/catalog values; no hardcoded model inventory. Existing Claude/OpenCode defaults remain unchanged; Codex may initially inherit its own configured model/effort rather than expanding the existing provider-specific settings schema.

## Evidence inspected

- `apps/cockpit/src/agents.rs` registers only `claude` and `opencode`.
- `config/bin/.local/bin/greywall-agents/codex` exists but is a six-line launcher which does not source `launcher-common.sh`.
- No native Codex agent helper or versioned greywall profile exists in this checkout.
- Host-approved read-only help confirmed **codex-cli 0.161.0**. Interactive launch accepts a prompt and `--cd`, `--model`, `--no-daemon`, `--remote`; resume accepts an exact session identifier. Never use `--last` to identify a managed session.
- Installed `codex queue --help` confirms `queue --thread <THREAD> --message <TEXT>` and optional `--remote`. Help does not establish receipt/idempotency, queue wake or sandbox routing guarantees.
- Official hooks documentation defines `SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `PostToolUse`, `Stop`, `Interrupt` and `SessionEnd`; hook trust must be reviewed by the user. Background hook completion does not wake an idle session.
- Official app-server documentation describes native TUI `--remote` over a private Unix endpoint and JSON-RPC lifecycle. App-server is experimental; do not expose an unauthenticated TCP listener or assume a version-independent protocol.

Sources: <https://developers.openai.com/codex/hooks.md>, <https://developers.openai.com/codex/app-server/>. Implementation must verify the installed version/schema instead of relying on changing upstream `main` examples.

## Proposed technical choices

- Add a `codex` row/helper/profile using the existing agent contract, without a new agent framework.
- Use namespaced cockpit IDs `cdx_` plus the lowercase 64-character SHA-256 of the exact native thread ID. Store the native thread ID only in the verified binding. This avoids UUID collisions with Claude and keeps the native thread distinct from any parent/root session ID. Display native ID in details; use cockpit ID for routing and approval.
- Reports authenticate by peer credentials and the same launcher ancestry checks as the other agents. Native hook payloads are translated by a Codex-specific bridge, not by pretending a Codex UUID belongs to Claude. Child/subagent lifecycle cannot replace its parent's cockpit card.
- Rework the Codex launcher to use shared project guards, CA/proxy settings, project answer paths, clipboard bridge and current checkout-specific FigCraft environment. Maintain existing sandbox restrictions. Never copy the Claude/OpenCode profile wholesale or enable auto-learning during ordinary managed launches.
- Bootstrap a minimal versioned Codex profile from verified execution needs. If current policy blocks a required path/host, report the blocker and seek approval rather than granting a broad allow. Isolate Codex state and credentials from other agents and keep new runtime endpoints private to that launch.
- Subsequent explicit instruction: Theo requested the same sandbox profile as Claude/OpenCode rather than a new minimal policy. Use the shared global protections and existing OpenCode network allowlist, adapting owned paths for native Codex. Do not grant access to other agents' credentials, add new network destinations silently or enable learning. Native auth hosts absent from the existing allowlist remain an explicit approval prerequisite if needed.
- Prefer lifecycle hooks for reports. Install a dedicated Codex hooks source without overwriting existing user hooks/config. Changed hooks remain awaiting the user's native trust review; never use `--dangerously-bypass-hook-trust`, auto-trust or invent a trust database edit.
- Share the existing report socket, not a new sandbox-visible host command socket. Native ask-to-run and native Codex permission dialogs remain distinct approval surfaces. Reporting a native permission request never answers it.
- For native prompt delivery, investigate `codex queue` against the exact managed runtime. If it uses a shared daemon incompatible with namespace isolation, use a private app-server Unix endpoint only after proving the same TUI/session connection and bounded authentication. The app-server, bridge and all Codex execution remain inside greywall; no server runs product code with host privileges.
- Existing FigCraft use must connect through the reserved checkout port and normal explicit connection workflow. Do not hardcode a port, share another session's instance or configure permissionless access. If native MCP config needs modification, preserve unrelated user configuration and test the merge.

## Safety and completion gates

Before implementation, verify CLI flags and hook schema against the installed binary and capture minimal synthetic fixtures. Read help only; do not start a real billed turn merely to explore.

Before marking native integration complete, test the native entrypoints in a private fixture runtime and then perform an explicitly approved host smoke test. Launch failure, unknown status, missing hook trust, server loss and invalid session mapping must fail visibly; no automatic approval or silent unsandboxed fallback.

The main collaboration plan separately proves safe idle wake/busy queue delivery, durable deduplication and revocation. Native Codex registration alone does not pass that gate. Likewise, adding Codex must not weaken the guarantees for Claude Code CLI or OpenCode.

No installs, dependency/manifest edits, login changes, account migration, global policy weakening or automatic host reloads. Requested completed changes use the repository's host-reviewed patch/apply workflow. Files changed under launchers, hooks, shared host helpers and greywall config execute with host rights and must be called out during review.
