# Cockpit M1 live acceptance: sleep after idle

## Sleep after idle

1. Approve the command below. It starts the scratch daemon (it says so if it already runs).
   run: bash tools/acceptance/run.sh start
2. Approve the command below. It opens the TUI in an opaque window.
   run: bash tools/acceptance/run.sh tui-window
3. Approve the command below. It starts the idle session: Claude replies OK once, then does nothing.
   run: bash tools/acceptance/run.sh session new --prompt "Reply OK, then wait."
4. Look at the TUI: a new card appears under demo and Claude replies OK. Do not touch that session from now on. You can do something else now: come back in 30 minutes, then go on to the next step. This panel waits for you.
   ask: What time did Claude reply OK?
5. At least 30 minutes after Claude replied, approve the command below. It shows the idle session.
   run: bash tools/acceptance/run.sh session show
   ask: Does it say asleep, and does its card in the TUI say asleep too? If not, how many minutes since Claude replied?
6. In the TUI, click the list, select that card (j/k) and press Enter: that wakes it. Click into the viewer and send Claude the message below.
   copy: What did you reply before?
   ask: Did it wake, keep its conversation and answer OK?
7. Approve the command below. It stops the scratch daemon and its sessions. The scratch directory stays as evidence.
   run: bash tools/acceptance/run.sh stop
8. Close the TUI window.
   ask: Anything else worth noting? (none is a fine answer)
