# Prove native instruction input through the real daemon before signed-in validation

Issue #419. Implemented and measured on 2026-11-02.

The remaining issue scope is a live model seeing the brief and listing the native
skill where supported. #469 and #481 already implement delivery and native skill
wiring; this change adds evidence without reimplementing them. Total context moved
to #479.

Use the existing temporary-home daemon fixture and its fake managed sandbox
engines, replacing only the provider executables with isolated native wrappers.
Owner `SessionNew`, provider launch, hook/plugin execution, `cockpit brief`, native
skill loading, MCP discovery and the authenticated agent channel are real. A local
HTTP model records the outbound native input before any model-generated instruction
can influence it. The neutral first prompt does not mention Cockpit or the brief.

Measured versions are Claude Code 2.1.316, Codex 0.166.0 and the separately approved
OpenCode 1.19.5 extraction. Claude/OpenCode match their target pins. Codex's target
remains 0.161.0; its native observation does not expand that capability provenance.
The installed OpenCode 2.1.4 is not used as evidence for 1.19.5.

Claude uses native print mode with explicit project settings and the real
additional skill directory. Disabling project settings would suppress that skill
discovery. Its exact 2.1.316 request puts SessionStart additional context and skill
advertisements in a `messages` entry with role `system`, and defers Cockpit MCP
schemas behind `ToolSearch`. Codex's actual TUI/app-server puts the brief in
developer input and advertises MCP tools, with native Cockpit skill support still
explicitly absent. OpenCode's actual TUI puts the brief and skill list in system
input and advertises MCP tools.

Fresh environment/config/state and metadata-only refusal of external policy
prevent accidental owner configuration use. Native commands are bounded and
temporary OpenCode permissions are restored. No account access, model usage,
installation, new dependency or production behavior change was needed.

Rejected: running the existing Claude `live_hooks` probe as an isolation test (it
inherits the owner's home and uses a real model); copying authentication into a
fixture; treating canned model text as evidence of comprehension; changing Codex's
production version pin; claiming real sandbox behavior from the fake engine.

The signed-in acceptance remains open. Before such a run, prepare a separate
reviewable command with explicit account usage and host effects and obtain host
approval through `ask-to-run`. This change supplies no live toggle. Claude TUI
parity, signed-in model recognition, Codex 0.161.0 native proof and #479 total
context remain unmeasured. OpenCode persisted completion remains #516.

Evidence and exact execution instructions:
[native instruction fixture](../../crates/cockpit-daemon/tests/instruction_fixture/README.md).
