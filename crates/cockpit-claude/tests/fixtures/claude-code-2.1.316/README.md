# Claude Code 2.1.316 hook payloads

One payload per hook event the adapter subscribes to, as Claude Code 2.1.316 writes it on
the hook's standard input. They follow the hook input schema the legacy hook
(`config/claude/.claude/hooks/session-state.sh`) consumed on 2.1.31x; they were written from
that schema, not captured from a live run. Capturing them live is the `validate` step of the
provider conformance suite (#51).

`cockpit-claude`'s `CONFORMANCE_VERSION` names this directory; a new pinned version gets a
new directory and the tests move to it.
