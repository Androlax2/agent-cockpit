// An opencode session on the cockpit's column, like a Claude one: its events go to the same hook
// as Claude's (~/.claude/hooks/session-state.sh), as the payloads Claude Code sends, with
// CLAUDE_SESSIONS_AGENT=opencode. The hook works out the turn's outcome inside the sandbox and
// reports to the OSD daemon.
import type { Plugin } from "@opencode-ai/plugin"
import { spawn } from "node:child_process"
import { homedir } from "node:os"

const HOOK = `${homedir()}/.claude/hooks/session-state.sh`

type HookPayload = { hook_event_name: string } & Record<string, unknown>

export const CockpitPlugin: Plugin = async ({ client, directory }) => {
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
      if (code !== 0) {
        await client.app.log({ body: { service: "cockpit", level: "error", message: `session-state.sh ${payload.hook_event_name} exited ${code}: ${error}` } })
      }
    })
  }

  // A resumed session (opencode --session) creates nothing: it starts at the first event seen.
  const report = (sessionID: string, payload: HookPayload) => {
    const root = rootOf(sessionID)
    if (!started.has(root)) {
      started.add(root)
      if (payload.hook_event_name !== "SessionStart") send(root, { hook_event_name: "SessionStart" })
    }
    send(root, payload)
  }

  return {
    event: async ({ event }) => {
      // The plugin package still types its events after the v1 API: match on the names alone.
      const { type, properties } = event as { type: string; properties: any }
      switch (type) {
        case "session.created":
          if (properties.info.parentID) {
            parentOf.set(properties.info.id, properties.info.parentID)
          } else {
            report(properties.info.id, { hook_event_name: "SessionStart" })
          }
          break
        case "session.idle":
          if (!parentOf.has(properties.sessionID)) report(properties.sessionID, { hook_event_name: "Stop" })
          break
        case "session.deleted":
          if (!parentOf.has(properties.info.id) && started.delete(properties.info.id)) {
            send(properties.info.id, { hook_event_name: "SessionEnd" })
          }
          break
        case "permission.asked":
        case "permission.updated": // before opencode 1.x renamed it
          report(properties.sessionID, {
            hook_event_name: "Notification",
            notification_type: "permission_prompt",
            message: `opencode needs your permission: ${properties.permission ?? properties.title ?? "a tool"}`,
          })
          break
        case "question.asked":
          report(properties.sessionID, { hook_event_name: "Notification", notification_type: "elicitation_dialog", message: "opencode has a question for you" })
          break
      }
    },

    "chat.message": async (input, output) => {
      if (parentOf.has(input.sessionID)) return
      const prompt = output.parts.flatMap((part: any) => (part.type === "text" && !part.synthetic ? [part.text] : [])).join("\n")
      report(input.sessionID, { hook_event_name: "UserPromptSubmit", prompt })
    },

    "tool.execute.after": async (input) => {
      report(input.sessionID, { hook_event_name: "PostToolUse", tool_name: input.tool })
    },

    // ask-to-run, ask-to-guide and claude-todo name the session their request comes from.
    "shell.env": async (input, output) => {
      if (input.sessionID) output.env.CLAUDE_CODE_SESSION_ID = rootOf(input.sessionID)
    },
  }
}
