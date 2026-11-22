# Claude Code loads Torlek's skills as a `--plugin-dir` plugin, never as an `--add-dir` folder

Status: accepted

Issue #1419. Decided by Theo, from three options the implementing session measured.

- **Every launch passes `--plugin-dir <state>/claude-plugin`.** From 2.1.326 Claude Code shows
  a trust prompt for a folder added with `--add-dir` that the owner has not trusted, and the
  answer lasts one session (code.claude.com/docs/en/changelog, 2.1.326;
  code.claude.com/docs/en/plugins/loading: "Answering the prompt that `--add-dir` shows
  doesn't save that trust"), so every unattended start waited on it. On 2.1.330 a plugin
  loaded with `--plugin-dir` lists its skills with no prompt. The docs say nothing about
  trust for `--plugin-dir`, so that rests on this measurement and on the fake `claude` in
  `cockpit-daemon` `tests/real_daemon`, which stops on `--add-dir` like 2.1.326.
- **The daemon writes `.claude-plugin/plugin.json` naming the plugin `torlek`**, and the bundle
  and the skill pack in its `skills/` (code.claude.com/docs/en/plugins/create). Claude Code
  lists them as `/torlek:<skill>`; the owner accepted the prefix. Torlek writes nothing in
  Claude Code's configuration for this.
- **A Claude Code without `--plugin-dir` fails its version probe naming the flag**, so readiness
  says so (`cockpit-daemon`
  `readiness::a_claude_code_without_plugin_dir_is_reported_naming_the_flag`).
- **Rejected:** `torlek setup` writing `projects["<state>/claude-skills"].hasTrustDialogAccepted`
  into `~/.claude.json` (documented in code.claude.com/docs/en/permissions, but it edits the
  owner's Claude Code configuration, which running instances also rewrite, and trusting the
  folder also lets its settings and git configuration apply); asking the owner to trust the
  folder by hand once per machine; `permissions.additionalDirectories`, which grants file
  access only and loads no skills (code.claude.com/docs/en/permissions).

Evidence: `cockpit-daemon` `sandbox::a_claude_launch_lists_the_torlek_skill_without_a_trust_prompt`,
`sandbox::a_sandboxed_claude_reads_the_skill_of_the_plugin_its_launch_loads`; `cockpit-claude`
`launch::a_launch_loads_the_instruction_plugin_so_claude_lists_the_torlek_skill`,
`launch::the_plugin_manifest_names_the_plugin_torlek`.
