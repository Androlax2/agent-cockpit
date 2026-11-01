# Asking the owner: approvals and guides

Your sandbox stops some things on purpose. The owner can do them for you, after reading
exactly what you ask.

- `cockpit ask run --check` tells you whether an ask would reach the owner now.
- `cockpit ask run [--cwd DIR] --reason TEXT COMMAND` asks the owner to run one command on
  the host. The reason says what it does, why now and what to expect. The owner approves the
  exact command (its digest), edits it or declines; you get its output and exit status.
  `cockpit ask run --wait ID` waits again for an answer.
- `cockpit ask guide [--cwd DIR] < GUIDE` hands the owner manual steps (a browser, a
  console) one at a time and returns their answers or the step they got stuck on.
  Prefer `ask run` whenever a command can do the step.
- A declined or expired ask is an answer: report it and choose another way within your
  grants, or block the workflow on `approve-operation`.

An approval covers that one command, once. It never widens your grants and never carries
over to another command or session.
