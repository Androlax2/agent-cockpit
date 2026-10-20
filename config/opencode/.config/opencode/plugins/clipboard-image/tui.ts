import { createConnection } from "node:net"
import { mkdtemp, writeFile, rm, rmdir } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { pathToFileURL } from "node:url"

const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])
const MAX_IMAGE_BYTES = 20 * 1024 * 1024
const READ_TIMEOUT_MS = 2000
const STAGED_IMAGE_TTL_MS = 60_000

function readImage(socketPath: string, signal: AbortSignal): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(socketPath)
    const chunks: Buffer[] = []
    let totalBytes = 0
    let hasSettled = false
    const finish = (error?: Error) => {
      if (hasSettled) return
      hasSettled = true
      clearTimeout(deadline)
      signal.removeEventListener("abort", abort)
      socket.destroy()
      if (error) reject(error)
      else resolve(Buffer.concat(chunks, totalBytes))
    }
    const abort = () => finish(new Error("Image paste cancelled"))
    const deadline = setTimeout(() => finish(new Error("Image clipboard bridge timed out")), READ_TIMEOUT_MS)
    signal.addEventListener("abort", abort, { once: true })
    if (signal.aborted) abort()
    socket.on("error", () => finish(new Error("Image clipboard bridge unavailable")))
    socket.on("data", (chunk: Buffer) => {
      totalBytes += chunk.length
      if (totalBytes > MAX_IMAGE_BYTES) {
        finish(new Error("Clipboard image exceeds 20 MiB"))
        return
      }
      chunks.push(chunk)
    })
    socket.on("end", () => finish())
  })
}

export default {
  id: "cockpit-image-paste",
  setup(context: any) {
    let isDisposed = false
    let pendingPaste: Promise<void> | undefined
    let activeRead: AbortController | undefined
    const staged = new Map<string, ReturnType<typeof setTimeout>>()
    const socketPath = join(process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid!()}`, "clipboard-image", "clipboard.sock")

    const removeImage = async (file: string) => {
      const timer = staged.get(file)
      if (!timer) return
      clearTimeout(timer)
      staged.delete(file)
      await rm(file, { force: true })
      await rmdir(dirname(file))
    }
    const routeKey = () => {
      const route = context.ui.router.current()
      return JSON.stringify([route.type, route.sessionID])
    }
    const unregister = context.ui.slot({
      append: "prompt.footer",
      render(input: { mode: string }) {
        const canPaste = () => {
          const focused = context.renderer.currentFocusedRenderable
          // Only the composer expands attachment placeholders; other text inputs keep their paste.
          return !isDisposed && input.mode === "normal" && context.keymap.mode.current() === "base"
            && typeof focused?.plainText === "string" && typeof focused.getClipboardText === "function" && !focused.isDestroyed
        }
        const paste = async () => {
          const focused = context.renderer.currentFocusedRenderable
          const before = { text: focused.plainText, route: routeKey() }
          const hasPromptChanged = () => isDisposed || !canPaste() || context.renderer.currentFocusedRenderable !== focused
            || focused.plainText !== before.text || routeKey() !== before.route
          activeRead = new AbortController()
          let stagedFile: string | undefined
          try {
            const image = await readImage(socketPath, activeRead.signal)
            if (isDisposed) return
            if (hasPromptChanged()) throw new Error("Image paste cancelled because the prompt changed")
            if (image.length === 0) throw new Error("No PNG image in the clipboard")
            if (!image.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) throw new Error("Clipboard bridge did not return a PNG image")
            const directory = await mkdtemp(join(tmpdir(), "opencode-image-"))
            stagedFile = join(directory, "clipboard.png")
            // The native paste handler reads the file asynchronously and converts it to a data URI.
            const file = stagedFile
            const timer = setTimeout(() => {
              void removeImage(file).catch((error) => console.error("image paste cleanup:", error))
            }, STAGED_IMAGE_TTL_MS)
            timer.unref()
            staged.set(file, timer)
            await writeFile(file, image, { mode: 0o600, flag: "wx" })
            if (hasPromptChanged()) throw new Error("Image paste cancelled because the prompt changed")
            context.renderer.keyInput.processPaste(Buffer.from(pathToFileURL(file).href))
          } catch (error) {
            if (stagedFile) await removeImage(stagedFile)
            if (!isDisposed) context.ui.toast.show({ message: error instanceof Error ? error.message : "Image paste failed", variant: "error" })
          } finally {
            activeRead = undefined
          }
        }
        context.keymap.layer(() => ({
          mode: "base",
          priority: 100,
          target: () => context.renderer.currentFocusedRenderable,
          enabled: canPaste,
          commands: [{
            id: "cockpit.image-paste",
            title: "Paste clipboard image",
            group: "Prompt",
            bind: "ctrl+v",
            run() {
              if (!canPaste() || !context.keymap.commands().some((command: { id: string }) => command.id === "prompt.paste")) return false
              if (pendingPaste) return pendingPaste
              pendingPaste = paste().finally(() => { pendingPaste = undefined })
              return pendingPaste
            },
          }],
        }))
        return null
      },
    })
    return async () => {
      isDisposed = true
      unregister()
      activeRead?.abort()
      await pendingPaste
      await Promise.all([...staged.keys()].map(removeImage))
    }
  },
}
