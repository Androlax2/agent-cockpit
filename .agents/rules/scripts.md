# Scripts and integrations

- Discover executable scripts by tracked paths and shebangs, not just extensions.
  `config/bin/.local/bin` contains extensionless Python, POSIX shell and Bash commands.
- Keep each script's declared dialect. Fish and Lua are not ShellCheck input; browser
  JavaScript and Node tooling have different globals. OpenCode plugins also use TypeScript.
- Quote paths and preserve argument boundaries. Test filenames with spaces when changing
  command discovery or subprocess invocation.
- Fail clearly on missing tools and invalid input. A check must not install its own
  dependencies or silently skip files it cannot validate.
- Scripts, Make recipes, hooks and systemd configuration may run on the host with the
  user's rights. Explicitly identify changes to them in the implementation summary.
- Use the existing fake-command test fixtures. Never test an installation, restart,
  Git mutation or credential operation against the real host without its approval flow.

See [Architecture](../../docs/ARCHITECTURE.md) for source and test owners.
