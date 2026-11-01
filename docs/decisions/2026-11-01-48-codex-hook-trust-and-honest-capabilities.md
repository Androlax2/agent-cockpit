# Codex hooks run with trust scoped to the launch, and the adapter reports resume and events only

Issue #48. Decided by the worker: Codex 0.161.0 runs no hook without recorded trust, and its
failed turns send no hook (both measured with a fresh `CODEX_HOME`).

- **Each launch trusts exactly its own hooks.** The launch passes the hooks with `--config`
  and a `hooks.state` entry holding each hook's trusted hash, computed as Codex does
  (`cockpit-codex` `hooks::trusted_hash`, proven by `conformance::codex_trusts_every_hook_a_launch_subscribes`).
  Codex files both under its session-flags source, so the trust lives and dies with the
  conversation, and a hook whose command differs is not run.
- **Rejected:** `--dangerously-bypass-hook-trust`, because it also runs every untrusted hook a
  cloned project declares; writing trust into the owner's `~/.codex/config.toml`, because it
  changes the owner's files and outlives the launch.
- **`--no-daemon` on every launch.** Codex's shared background server runs hooks with the
  environment of whichever launch started it, so reports would carry another session's launch
  token (the #184 failure for Claude Code).
- **Capabilities: resume, and events with the bridge.** Submit and cancel need the app-server
  delivery decided in #407. Outcome is not reported: an API error ends the turn without a
  `Stop` hook, so a workflow keyed on `outcome` would wait forever. Effort, compact, billing
  and screen reading are untested.
- **Rejected:** typing into the Codex TUI for submit and cancel, which #48 replaces with the
  app-server RPC.
