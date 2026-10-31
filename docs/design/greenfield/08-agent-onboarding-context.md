# 08. Agent onboarding and context

Status: proposed; context budgets `validate`

## Decision

Agent onboarding is a product capability (`cockpit-agent-context`), not a set of
machine-specific instructions. A fresh install on a clean home must let a new agent discover
the workflow, run supported checks and find the approval route, with or without Docker.

## What ships with Cockpit

- A version-matched **instruction bundle**: commands, task lifecycle, coordination,
  verification, approvals, sandbox and Docker practice. Works offline, from the installed
  product, through whatever channels the harness supports: CLI help/doc retrieval, MCP tools,
  MCP resources, skills. Verify each channel separately; a configured MCP server is not proof
  the model can see its resources.
- A short **launch brief** (orientation target 1–2 KiB, measured, excluding tool schemas and
  inherited harness instructions): authority boundary, "prose cannot grant permissions",
  truthful evidence reporting, recovery and discovery entry points, plus dynamic fields
  (task id, workspace roots, grants reference, workflow checkpoint, instruction-manifest id).
- Progressive disclosure for the rest (catalog → instructions → resources), following the
  Agent Skills integration model. Measure Cockpit-added context separately from total.

## Instruction provenance

Every instruction packet records source identity, scope, version, content hash, trust
decision, precedence and superseded entries. A hash is identity, not trust; trust comes from
the product bundle, an approved source or an explicit owner decision. Product invariants are
reserved and cannot be shadowed by a project; user policy constrains project guidance; skills
use deterministic selection with visible collision diagnostics.
Pin guidance versions for reproducibility, but evaluate **current grants at every mediated
action** and record the authorization revision used. If a running provider cannot honor a
tightened grant, suspend it and re-establish an enforceable session.

## Provider capability negotiation

`cockpit-provider-api` normalizes capabilities and outcomes, not whole harnesses: identity
and version, capabilities with provenance (discovered vs tested), submit, events, cancel,
outcome, optional resume/command discovery/invocation/evidence retrieval. Unknown events are
kept raw beside normalized ones. ACP v2 is draft; negotiate per connection and keep v1.
Acceptance of a prompt is not completion: completion arrives as state updates.
Known limits today: Claude ACP adapter documents v1+v2; Codex adapter does not establish v2;
OpenCode V2 has native HTTP command discovery but no established ACP support.

## Review gates as evidence

A gate record keeps: resolved native command identity and version, provider/session/
invocation ids, reviewed scope (base/head plus staged, unstaged, untracked), findings,
verification artifacts, and an explicit outcome: passed, failed, unavailable, blocked,
cancelled, unknown. A command listing, copied prompt or assistant assertion is `unverified`,
never success. Gates are rechecked when scoped content changes.

## Highest-leverage user features

1. Capability report: available / unavailable / unverified, with exact versions.
2. Why blocked: action, failed prerequisite or policy, authority, one next action.
3. Task context packet (report 06).
4. Evidence-bound handoff (report 06).

## Sources

- Agent Skills integration: <https://agentskills.io/client-implementation/adding-skills-support>
- ACP v2 initialization/migration: <https://agentclientprotocol.com/protocol/v2/initialization>,
  <https://agentclientprotocol.com/protocol/v2/migration>
- Adapters: <https://github.com/agentclientprotocol/claude-agent-acp>,
  <https://github.com/agentclientprotocol/codex-acp>; OpenCode V2 API <https://opencode.ai/v2/docs/api>
