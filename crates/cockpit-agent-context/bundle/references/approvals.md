# Asking the owner: approvals and guides

Your sandbox stops some things on purpose. The owner can do them for you, after reading
exactly what you ask.

- `torlek ask run --check` tells you whether an ask would reach the owner now.
- `torlek ask run [--cwd DIR] --reason TEXT COMMAND` asks the owner to run one command on
  the host. The reason says what it does, why now and what to expect. The owner approves the
  exact command (its digest), edits it or declines; you get its output and exit status.
  `torlek ask run --wait ID` waits again for an answer.
  Asking again while the same command in the same directory is still pending files nothing:
  you get the pending id and `--wait ID`, and the new reason is dropped.
- An approved command runs confined: it writes only your project's checkout and a `/tmp` of
  its own, cannot see the owner's keys, tokens or Torlek's state, has no network and no sudo.
  Name what else it needs when you ask: `--write DIR` for each other folder it writes,
  `--host HOST` (`host` or `host:port`) for each host it reaches through the proxy. The owner
  approves them with the command. A limit the run hit is printed as `the run hit a limit`.
- The scripts, makefiles and other code files your command runs are hashed when you ask.
  Finish editing them first: one that changes before the run starts refuses it ("FILE
  changed since it was asked" or "changed after you approved"), and you ask again.
- `torlek ask guide [--cwd DIR] < GUIDE` hands the owner manual steps (a browser, a
  console) one at a time and returns their answers or the step they got stuck on.
  Prefer `ask run` whenever a command can do the step.
- A declined or expired ask is an answer: report it and choose another way within your
  grants, or block the workflow on `approve-operation`.

An approval covers that one command, once, unless the owner approves it always: then your
project's later asks of exactly that command line, directory, `--write` folders, `--host`
hosts and hashed files run without waiting for the owner. Any change asks again; there are no
patterns. An approval never widens your grants.
