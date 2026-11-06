# `torlek` alone answers at the first signed-in provider; every sign-in is still asked fresh

Issue #1048 (bare `torlek` took about a second). Decided by the worker: of the options
measured, it is the only fast one that never shows a sign-in older than the request, and Theo
asked to be consulted before that is traded away.

Measured on a fixture daemon whose provider commands sleep as the real ones did on Theo's host
on 2026-11-06 (`claude auth status` 125 ms, `codex login status` 50 ms, `opencode auth list`
910 ms): inside one readiness report the clipboard took 6 ms, the configuration 0.2 ms and the
three sign-ins 141 + 60 + 903 ms, one after the other.

- **The providers are asked together.** The report waits for the slowest status command, not
  for their sum: `torlek doctor` and `torlek init` went from 1113 ms to 908 ms on the fixture.
  Proven by `doctor_asks_the_providers_for_their_sign_in_at_the_same_time` in
  `crates/cockpit-daemon/tests/readiness.rs`.
- **`readiness` takes `is_one_sign_in_enough`.** With it the daemon answers when the first
  provider reports signed in; a provider that has not answered by then keeps the `unknown`
  "sign-in has not been probed" fact. With no provider signed in every one is waited for, so a
  missing sign-in is never hidden. Only `torlek` alone sets it: `doctor`, `init`, the status
  page and the support bundle ask for every sign-in as before.
- **Only one report's status commands run on after its answer.** They are then outside that
  report's bulk lane place, each bounded by its own 10 s deadline. While they run, the next
  such report waits for its own, as every report did before, so they cannot pile up: a second
  `torlek` alone within that second takes as long as `doctor` (913 ms on the fixture). Proven
  by `a_second_torlek_alone_waits_while_the_firsts_status_command_still_runs`.
- **`torlek` alone asks twice only when it must.** The first answer decides between the usage
  and the checklist. The checklist names each provider's own sign-in, so when the first answer
  holds a signed-in provider beside an `unknown` one (the only case in which the daemon may
  have answered early) it asks again for them all; what `torlek` alone prints is unchanged in
  every state. On the fixture, with the setup done: 1118 ms before, 71 ms with codex signed
  in, 152 ms with only claude signed in. Proven by the three `torlek_alone_*` tests in the same
  file and `cockpit_alone_asks_once_when_every_sign_in_answered` in
  `crates/cockpit-cli/tests/onboarding.rs`.
- **Rejected: asking the providers together and nothing else.** Bare `torlek` would still wait
  for opencode, about 900 ms.
- **Rejected: not asking for sign-ins from `torlek` alone**, or asking only when another step
  is open. About 6 ms, but once the setup is done `torlek` alone would print the usage with
  every provider signed out, where it shows the sign-in step today.
- **Rejected: keeping the last answers in the daemon.** Fast only while they are young, and
  `torlek` alone could then report a sign-in that was since lost. Theo's to choose if the
  remaining 70 to 150 ms matter: it is the stale-readiness trade.
