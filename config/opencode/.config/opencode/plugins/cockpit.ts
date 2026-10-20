// An opencode session on the cockpit's column, like a Claude one: its events go to the same hook
// as Claude's (session-state.sh), as the payloads Claude Code sends, with
// CLAUDE_SESSIONS_AGENT=opencode. The hook works out the turn's outcome inside the sandbox and
// reports to the OSD daemon.
//
// opencode 2's plugin API (packages/plugin/src/promise/plugin.ts): a default export with an id and
// a setup. The plugin runs in the session's server, which the cockpit starts private to its TUI
// (--standalone), so it lives and ends with the session. The cockpit picks the session's id
// (--session) and exports it as CLAUDE_CODE_SESSION_ID: the card shows from the start, before
// opencode makes the session at the first prompt, unless the launcher pre-creates it with defaults.
import { spawn } from "node:child_process"
import { homedir } from "node:os"

// Where ~/.claude/hooks points: inside an opencode sandbox ~/.claude stays hidden, and greywall makes
// no link there, only the folder the link points to.
const HOOK = `${homedir()}/dotfiles/config/claude/.claude/hooks/session-state.sh`

type HookPayload = { hook_event_name: string } & Record<string, unknown>

export default {
  id: "cockpit",
  setup: async (ctx: any) => {
    const directory: string = ctx.location?.directory ?? process.cwd()
    const cockpitSession = process.env.CLAUDE_CODE_SESSION_ID
    // A subagent runs in a session of its own, with a parent: what it asks shows on its root's card.
    const parentOf = new Map<string, string>()
    const started = new Set<string>()
    const rootOf = (sessionID: string): string => {
      let root = sessionID
      while (parentOf.has(root)) root = parentOf.get(root)!
      return root
    }

    // One report at a time, in the order of the events.
    let reports = Promise.resolve()
    const send = (sessionID: string, payload: HookPayload) => {
      reports = reports.then(async () => {
        // node's spawn, which Bun has too: the tests run this plugin under node.
        const hook = spawn("bash", [HOOK], { env: { ...process.env, CLAUDE_SESSIONS_AGENT: "opencode" }, stdio: ["pipe", "ignore", "pipe"] })
        let error = ""
        hook.stderr.on("data", (chunk) => (error += chunk))
        hook.stdin.end(JSON.stringify({ ...payload, session_id: sessionID, cwd: directory }))
        const code = await new Promise<number | null>((resolve) => {
          hook.on("close", resolve)
          hook.on("error", (spawnError) => {
            error = String(spawnError)
            resolve(null)
          })
        })
        if (code !== 0) console.error(`cockpit plugin: session-state.sh ${payload.hook_event_name} exited ${code}: ${error}`)
      })
    }

    // A session resumed or chosen by the cockpit may never send session.created: it starts at the
    // first event seen.
    const report = (sessionID: string, payload: HookPayload) => {
      const root = rootOf(sessionID)
      if (!started.has(root)) {
        started.add(root)
        if (payload.hook_event_name !== "SessionStart") send(root, { hook_event_name: "SessionStart" })
      }
      send(root, payload)
    }

    if (cockpitSession?.startsWith("ses")) report(cockpitSession, { hook_event_name: "SessionStart" })

    const isTopLevel = (sessionID: string) => !parentOf.has(sessionID)
    const handle = (event: { type: string; data: any }) => {
      const { type, data } = event
      switch (type) {
        case "session.created":
          if (data.parentID) parentOf.set(data.sessionID, data.parentID)
          else report(data.sessionID, { hook_event_name: "SessionStart" })
          break
        case "session.inbox.enqueued":
          if (data.item?.type === "user" && isTopLevel(data.sessionID)) {
            report(data.sessionID, { hook_event_name: "UserPromptSubmit", prompt: data.item.payload?.text ?? "" })
          }
          break
        case "session.execution.succeeded":
        case "session.execution.failed":
        case "session.execution.interrupted":
          if (isTopLevel(data.sessionID)) report(data.sessionID, { hook_event_name: "Stop" })
          break
        case "session.deleted":
          if (isTopLevel(data.sessionID) && started.delete(data.sessionID)) send(data.sessionID, { hook_event_name: "SessionEnd" })
          break
        case "permission.asked":
          report(data.sessionID, {
            hook_event_name: "Notification",
            notification_type: "permission_prompt",
            message: `opencode needs your permission: ${data.message ?? data.action ?? "a tool"}`,
          })
          break
        case "form.created":
          report(data.form.sessionID, { hook_event_name: "Notification", notification_type: "elicitation_dialog", message: "opencode has a question for you" })
          break
      }
    }

    await ctx.tool.hook("execute.after", (input: { tool: string; sessionID: string }) => {
      report(input.sessionID, { hook_event_name: "PostToolUse", tool_name: input.tool })
    })

    // Linked sessions: pull this session's next approved team message and admit it natively,
    // queued behind the current turn. The stable inbox id makes a repeated admission return the
    // original instead of a second turn. Only the cockpit's own root session takes them.
    const runTeam = (args: string[], input?: unknown) =>
      new Promise<string>((resolve, reject) => {
        const child = spawn("cockpit-team", args, { stdio: ["pipe", "pipe", "pipe"] })
        let output = ""
        let error = ""
        child.stdout.on("data", (chunk) => (output += chunk))
        child.stderr.on("data", (chunk) => (error += chunk))
        child.on("error", reject)
        child.on("close", (code) => (code === 0 ? resolve(output) : reject(new Error(error.trim() || `cockpit-team ${args[0]} exited ${code}`))))
        // A command that exits without reading its input must not crash opencode's server (EPIPE).
        child.stdin.on("error", (stdinError: NodeJS.ErrnoException) => {
          if (stdinError.code !== "EPIPE") reject(stdinError)
        })
        if (input === undefined) child.stdin.end()
        else child.stdin.end(JSON.stringify(input))
      })
    const receipt = (dispatchID: string, outcome: Record<string, string>) =>
      runTeam(["receipt"], { action: "receipt", dispatch_id: dispatchID, outcome }).catch((error) =>
        console.error("cockpit plugin: team receipt not recorded", error),
      )
    const deliverTeamMessage = async () => {
      const delivery = JSON.parse((await runTeam(["receive"])).trim() || "null")
      if (!delivery?.dispatch_id) return
      const inboxID = `msg_cockpit${delivery.message_id}`
      try {
        const admitted = await ctx.session.synthetic({
          sessionID: cockpitSession,
          id: inboxID,
          text: delivery.text,
          description: "Cockpit team message",
          metadata: { cockpit_team: { message_id: delivery.message_id, dispatch_id: delivery.dispatch_id } },
          delivery: "queue",
          resume: true,
        })
        await receipt(delivery.dispatch_id, { outcome: "received", native_id: admitted?.id ?? inboxID })
      } catch (error) {
        // The inbox may have taken it before failing: never resend, let Theo reconcile.
        await receipt(delivery.dispatch_id, { outcome: "uncertain", reason: String(error).slice(0, 300) })
      }
    }
    const teamPoll = Number(process.env.COCKPIT_TEAM_POLL_MS ?? 3000)
    let teamBusy = false
    let teamStopped = false
    const teamTimer = cockpitSession?.startsWith("ses")
      ? setInterval(() => {
          if (teamBusy || teamStopped) return
          teamBusy = true
          deliverTeamMessage()
            .catch((error) => {
              if ((error as NodeJS.ErrnoException)?.code === "ENOENT") teamStopped = true
              else console.error("cockpit plugin: team delivery unavailable", error)
            })
            .finally(() => (teamBusy = false))
        }, teamPoll)
      : undefined

    // Not awaited: setup must return for opencode to take prompts.
    const subscription = new AbortController()
    void (async () => {
      for await (const event of ctx.event.subscribe({ signal: subscription.signal })) handle(event)
    })().catch((error: unknown) => {
      if (!subscription.signal.aborted) console.error("cockpit plugin: event stream ended", error)
    })
    return () => {
      subscription.abort()
      if (teamTimer) clearInterval(teamTimer)
    }
  },
}
