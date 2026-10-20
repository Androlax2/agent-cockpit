# ADHD-friendly mobile session chat

## Approved direction

The phone interface becomes a conversation, not a miniature terminal. Fix replying to ordinary agent questions. Keep the desktop interface unchanged. The user additionally requested the simplest possible mobile experience for answering messages and handling commands, explicitly ADHD-friendly.

## User experience

- Session list: project, plain-language status, and one short preview. Put sessions needing an answer first. Keep existing navigation available without adding more tabs.
- Session detail: compact project/status header, chronological You/Agent messages, and an always-reachable reply composer. One primary action, Send (Queue while working). Preserve drafts separately for each session, focus, and reading position across refreshes.
- Asking to run a command appears in its session's conversation as an action card: reason, exact command, Run and Decline. Preserve the existing second-tap Run confirmation and exact-command validation. Mark pending, running and finished states plainly when the existing backend provides them; do not invent completion evidence.
- Keep cross-session pending approvals discoverable on the session list, but do not show unrelated approvals above an open conversation.
- Hide terminal output, interrupt, remote-control and notification controls behind secondary disclosures. Terminal output remains available as a diagnostic fallback, never presented as reconstructed chat messages.
- No unsolicited auto-navigation, pulsing, decorative motion, or stream of tool logs in the main conversation. Status words must accompany colors.

## Implementation decisions proposed by the agent

Use the current Rust backend and vanilla JavaScript/CSS; no dependencies or manifest changes. Read existing Claude/OpenCode conversation storage read-only, not screenshots or ANSI parsing. Show user/assistant text only; omit reasoning, internal tool payloads and hidden/system messages. Clearly indicate unavailable history instead of making up a conversation from card summaries. Bound reads and response sizes, and restrict conversation access to active sessions behind existing phone authentication.

Ordinary chat questions (`needs_input`) must be replyable without weakening the refusal to inject text into actual permission or structured-question dialogs. Preserve the existing agent-specific permission capability checks.

The existing dark palette remains. Use readable wrapping text, visibly separated speaker turns, labeled inputs, keyboard focus indicators, touch targets of at least 44px, safe-area spacing, and an accessible terminal disclosure. Do not force-scroll readers who have scrolled up.

## Non-goals

No arbitrary remote shell, new command-execution API, automatic approvals, general desktop redesign, new provider integration, dependency installation, or broad cockpit rename.

## Acceptance

An ordinary agent question can be answered from the phone. Real permission dialogs remain protected. Conversations survive polling and session switches without leaking drafts or displaying stale responses from another session. Command approval needs no terminal navigation. Reading and replying works in a narrow mobile viewport with a keyboard open.
