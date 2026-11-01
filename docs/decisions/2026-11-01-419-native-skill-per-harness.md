# Claude Code and OpenCode list the bundle as a native `cockpit` skill; Codex does not

Issue #419. Decided by the implementing session: each harness's documented skill paths, read
on 2026-11-01, leave only these choices.

- **Claude Code: `--add-dir <state>/claude-skills`.** Claude Code loads the skills in the
  `.claude/skills/` of a directory given with `--add-dir` (code.claude.com/docs/en/skills,
  "Additional directory"). The daemon writes the bundle a second time at
  `<state>/claude-skills/.claude/skills/` at every start, a copy rather than a link because
  the documentation does not say links are followed. The sandbox may read that folder.
- **OpenCode: `skills.paths` in `OPENCODE_CONFIG_CONTENT`.** OpenCode 1.19.5 scans each
  folder in `skills.paths` for `**/SKILL.md` (`packages/opencode/src/skill/index.ts`, tag
  `v1.19.5`), so `$COCKPIT_INSTRUCTIONS` is added beside the owner's own paths; an owner's
  `skills` of another shape is refused, never overwritten.
- **Codex: no native skill.** Codex reads skills only from `.agents/skills` in the project
  and its parents, `$HOME/.agents/skills`, `/etc/codex/skills` and its own; no flag or
  config key adds a folder (`[[skills.config]]` only turns one off). Writing into the
  owner's home or the project would change files Cockpit does not own, so Codex agents keep
  the brief, `cockpit docs` and `cockpit mcp`.
- **Rejected:** a symbolic link from `.claude/skills/cockpit` to the bundle (unverified that
  Claude Code follows it), and laying the whole bundle out as `.claude/skills/` (OpenCode's
  scan skips dot folders below a skill path, and `$COCKPIT_INSTRUCTIONS` would change shape).

Evidence: `cockpit-claude` `launch::a_launch_adds_the_instruction_skills_root_so_claude_lists_the_cockpit_skill`,
`cockpit-opencode` `launch::a_launch_adds_the_instruction_bundle_to_the_owners_skill_paths`,
`cockpit-daemon` `sandbox::a_claude_launch_adds_the_instruction_skills_it_may_read`. A live
model listing the skill stays `validate` (needs signed-in harnesses).
