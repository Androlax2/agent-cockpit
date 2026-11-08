# A login account is signed in from the owner's terminal into a fresh folder, and only its file is kept

Status: accepted

Issue #59, part 3a. Running the harness's own sign-in in the owner's terminal, with a fresh
Torlek folder as its home, and moving only the login file was decided by Theo (2026-11-08,
option A); that a login is never copied from the owner's own folders is #858's rule. The rest
are engineering defaults listed in the pull request.

- **The owner's terminal runs the sign-in.** `torlek account sign-in PROVIDER ACCOUNT` asks the
  daemon for the adapter's sign-in command and a fresh home, then runs it there. A sandboxed
  sign-in was declined: a browser sign-in that calls back to a local port fails from inside
  the sandbox.
- **Only the login file is kept.** The adapter names the file (`Provider::login`, one of its
  credential files). The daemon checks it is a plain file inside the sign-in's home, moves it
  into `account-logins/<provider>/<account>/login/`, checks it again there where only the
  daemon writes, makes it owner-only and removes the rest of the home. It never opens the
  file. A sign-in that fails has its home removed, with any login it wrote.
- **The sign-in gets none of the owner's provider variables.** The CLI clears its environment
  and keeps what a terminal and a browser sign-in need (`PATH`, `TERM`, the locale, the
  display and session bus, `BROWSER`), so `CODEX_HOME`, `OPENAI_API_KEY` and their kin cannot
  send it to the owner's folders or a key.
- **The account must be listed.** A name `[accounts]` does not list for the provider is
  refused before anything runs.
- **Rejected:** copying the owner's own login file (it would share the owner's refresh token,
  T5 in #858); a sandboxed sign-in pane (local callbacks fail).

Evidence: `crates/cockpit-daemon/tests/account_sign_in.rs`, each adapter's
`its_login_is_a_credential_file_its_own_sign_in_saves`.
