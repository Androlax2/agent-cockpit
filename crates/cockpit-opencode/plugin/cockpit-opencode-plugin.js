// Cockpit's OpenCode plugin (crates/cockpit-opencode). A launch loads it through
// OPENCODE_CONFIG_CONTENT; the daemon writes this exact file, and its start check refuses any
// other content. It forwards the events that prove the agent's activity to
// cockpit-opencode-bridge, one at a time and in order, as JSON on its standard input. The
// bridge normalizes and reports them; this file only filters, so the per-event rules live in
// Rust (events.rs) beside their tests.
//
// It also adds the session's launch brief (`cockpit brief`, #419) to every system prompt, so
// the agent sees it without asking and a compaction never drops it.
//
// It stays silent unless it runs in the server its launch started: an `opencode` a tool
// starts inherits the environment, but not the port (COCKPIT_OPENCODE_PORT).
import { spawn } from "node:child_process"

// Must match events::FORWARDED_EVENTS.
const FORWARDED = [
  "session.created",
  "session.status",
  "permission.asked",
  "question.asked",
  "message.part.updated",
  "message.updated",
]

// How long `cockpit brief` may take before the prompt goes without it.
const BRIEF_DEADLINE_MS = 10000

// OpenCode's loop goes on after an assistant message that finished for these reasons.
const UNFINISHED = ["tool-calls", "unknown"]

export const CockpitBridge = async ({ serverUrl }) => {
  const bridge = process.env.COCKPIT_OPENCODE_BRIDGE
  const cockpit = process.env.COCKPIT_OPENCODE_COCKPIT
  const port = process.env.COCKPIT_OPENCODE_PORT
  let ownPort
  try {
    ownPort = String(serverUrl.port)
  } catch {
    return {}
  }
  if (!bridge || !cockpit || !port || ownPort !== port) return {}

  // Subagents run in child sessions, each mapped to its root session. Their activity is not
  // this conversation's, but a dialog they open waits for the owner in the same terminal.
  const roots = new Map()
  const rootOf = (id) => {
    while (roots.has(id)) id = roots.get(id)
    return id
  }
  // Assistant messages whose end was forwarded: OpenCode updates a message after it ends.
  const ended = new Set()
  let queue = Promise.resolve()

  const isForwarded = (event) => {
    const properties = event?.properties ?? {}
    if (!FORWARDED.includes(event?.type)) return false
    if (event.type === "session.created" && properties.info?.parentID) {
      roots.set(properties.info.id, properties.info.parentID)
      return false
    }
    if (roots.has(properties.sessionID)) return ["permission.asked", "question.asked"].includes(event.type)
    switch (event.type) {
      case "session.status":
        return ["busy", "idle"].includes(properties.status?.type)
      case "message.part.updated":
        return properties.part?.type === "tool" && ["completed", "error"].includes(properties.part?.state?.status)
      case "message.updated": {
        const info = properties.info ?? {}
        if (info.role !== "assistant" || !info.time?.completed || ended.has(info.id)) return false
        if (!info.error && (!info.finish || UNFINISHED.includes(info.finish))) return false
        ended.add(info.id)
        return true
      }
      default:
        return true
    }
  }

  const run = (payload) =>
    new Promise((resolve) => {
      try {
        const child = spawn(bridge, [cockpit], { stdio: ["pipe", "ignore", "ignore"] })
        child.on("error", resolve)
        child.on("close", resolve)
        child.stdin.on("error", () => {})
        child.stdin.end(payload)
      } catch {
        resolve()
      }
    })

  // What `cockpit brief` printed, asked once; a failed read is asked again on the next
  // prompt, and the prompt meanwhile goes without it rather than wait.
  let brief
  const readBrief = () =>
    new Promise((resolve) => {
      try {
        const child = spawn(cockpit, ["brief"], { stdio: ["ignore", "pipe", "ignore"] })
        const timer = setTimeout(() => child.kill("SIGKILL"), BRIEF_DEADLINE_MS)
        let text = ""
        child.stdout.setEncoding("utf8")
        child.stdout.on("data", (chunk) => {
          text += chunk
        })
        child.on("error", () => resolve(""))
        child.on("close", (code) => {
          clearTimeout(timer)
          resolve(code === 0 ? text.trim() : "")
        })
      } catch {
        resolve("")
      }
    })

  return {
    "experimental.chat.system.transform": async (_input, output) => {
      brief ??= readBrief()
      const text = await brief
      if (!text) {
        brief = undefined
        return
      }
      output.system.push(text)
    },
    event: async ({ event }) => {
      if (!isForwarded(event)) return
      const sessionID = event.properties?.sessionID
      const forwarded = roots.has(sessionID)
        ? { ...event, properties: { ...event.properties, sessionID: rootOf(sessionID) } }
        : event
      const payload = JSON.stringify(forwarded)
      queue = queue.then(() => run(payload))
    },
  }
}
