# Qwen Code 0.28.4 hook payloads

Written from Qwen Code's own definition of each payload at tag `v0.28.4` (commit
`e8424c88ddd463c56602063efde57c328b917cd4`): `packages/core/src/hooks/types.ts` and
`docs/users/features/hooks.md`. None was captured from a running Qwen Code: nothing was
installed on the host that wrote them (#1446). The session id, paths, model and token counts
are made up; the field names and values of `hook_event_name`, `source`, `notification_type`,
`error`, `trigger` and `reason` are Qwen Code's.

The live conformance test (`tests/conformance.rs`) reads an installed Qwen Code; replace a file
here with a captured payload once one is recorded.
