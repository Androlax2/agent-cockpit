//! The image-only TUI plugin driven through setup and its registered paste command, with a real
//! local clipboard socket. OpenCode's renderer is the boundary; no desktop clipboard is read.

mod common;
use common::sandbox::*;
use std::fs;

const HARNESS: &str = r#"
import assert from 'node:assert/strict'
import { createServer } from 'node:net'
import { mkdir, readdir, readFile, stat } from 'node:fs/promises'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const [plugin, scenario] = process.argv.slice(2)
const { default: definition } = await import(plugin)
const signature = Buffer.from([137,80,78,71,13,10,26,10])
const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64')
const socketPath = join(process.env.XDG_RUNTIME_DIR, 'clipboard-image', 'clipboard.sock')
await mkdir(dirname(socketPath), { recursive: true })
let connections = 0
let connected
const connectionReady = new Promise(resolve => connected = resolve)
let source
const server = createServer(socket => {
  connections++
  source = socket
  socket.on('error', () => {})
  if (scenario === 'focus-changed') context.renderer.currentFocusedRenderable = { plainText: 'another input', isDestroyed: false }
  if (scenario === 'prompt-changed') editor.plainText = 'newly typed text'
  if (scenario === 'route-changed') route = { type: 'session', sessionID: 'other' }
  connected()
  if (['timeout', 'unload-pending'].includes(scenario)) return
  if (scenario === 'empty') return socket.end()
  if (scenario === 'invalid') return socket.end('private copied text must not become an attachment')
  if (scenario === 'oversized') {
    const oversized = Buffer.alloc(20 * 1024 * 1024 + 1)
    signature.copy(oversized)
    return socket.end(oversized)
  }
  socket.end(image)
})
if (scenario !== 'unavailable') await new Promise(resolve => server.listen(socketPath, resolve))

let route = { type: 'session', sessionID: 'crema' }
const editor = { plainText: 'existing prompt', isDestroyed: false }
if (scenario !== 'no-prompt') editor.getClipboardText = text => text
const layers = []
const notices = []
const pasted = []
let unregistered = 0
const context = {
  renderer: {
    currentFocusedRenderable: editor,
    keyInput: { processPaste(bytes) {
      const uri = Buffer.from(bytes).toString('utf8')
      assert.ok(uri.startsWith('file://'), 'paste must use the native local-attachment path')
      const path = fileURLToPath(uri)
      assert.deepEqual(readFileSync(path), image)
      assert.equal(statSync(path).mode & 0o777, 0o600)
      assert.equal(statSync(dirname(path)).mode & 0o777, 0o700)
      pasted.push(path)
    } },
  },
  keymap: {
    layer(factory) { layers.push(factory) },
    commands: () => [{ id: 'prompt.paste' }],
    mode: { current: () => scenario === 'modal' ? 'modal' : 'base' },
  },
  ui: {
    slot(claim) {
      assert.equal(claim.append, 'prompt.footer')
      claim.render({ mode: scenario === 'shell' ? 'shell' : 'normal', sessionID: 'crema' })
      return () => unregistered++
    },
    router: { current: () => route },
    toast: { show(notice) { notices.push(notice) } },
  },
  client: new Proxy({}, { get() { throw new Error('image paste must never submit a prompt') } }),
}
const cleanup = await definition.setup(context)
assert.equal(layers.length, 1)
const layer = layers[0]()
assert.equal(layer.mode, 'base')
assert.equal(layer.commands[0].bind, 'ctrl+v')
assert.equal(layer.target(), editor)
const command = layer.commands[0]
if (['shell', 'modal', 'no-prompt'].includes(scenario)) {
  assert.equal(typeof layer.enabled === 'function' ? layer.enabled() : layer.enabled, false)
  await command.run()
  assert.equal(connections, 0)
} else if (scenario === 'unload-pending') {
  const pending = command.run()
  await connectionReady
  await cleanup()
  await pending
  assert.equal(pasted.length, 0)
  assert.equal(notices.length, 0)
} else {
  if (scenario === 'repeat') await Promise.all([command.run(), command.run()])
  else await command.run()
  if (['png', 'repeat'].includes(scenario)) {
    assert.equal(pasted.length, 1)
    assert.equal(connections, 1)
    assert.equal(editor.plainText, 'existing prompt', 'the plugin must not replace input or submit it')
  } else {
    assert.equal(pasted.length, 0, 'an invalid or stale paste must not enter an input')
    assert.ok(notices.some(notice => notice.variant === 'error' || notice.variant === 'warning'))
  }
}
if (scenario !== 'unload-pending') await cleanup()
for (const path of pasted) {
  assert.equal(existsSync(path), false)
  assert.equal(existsSync(dirname(path)), false)
}
assert.equal(unregistered, 1)
if (source) source.destroy()
if (scenario !== 'unavailable') await new Promise(resolve => server.close(resolve))
assert.equal((await readdir(process.env.TMPDIR)).length, 0, 'staged images must be cleaned up')
"#;

fn run(scenario: &str) {
    let sandbox = Sandbox::new();
    let plugin = repo().join("config/opencode/.config/opencode/plugins/clipboard-image/tui.ts");
    assert!(plugin.exists(), "the image-paste TUI plugin has not been implemented");
    let harness = sandbox.root.join("image-paste.mjs");
    let temporary = sandbox.root.join("private-tmp");
    fs::create_dir(&temporary).unwrap();
    fs::write(&harness, HARNESS).unwrap();
    let output = sandbox.command("node", &[]).env("TMPDIR", &temporary)
        .arg(harness).arg(plugin).arg(scenario).output().unwrap();
    assert!(output.status.success(), "{scenario}: {}", stderr(&output));
}

macro_rules! paste_test {
    ($name:ident, $scenario:literal) => {
        #[test]
        fn $name() { run($scenario); }
    };
}

paste_test!(an_image_is_staged_privately_and_pasted_without_submitting, "png");
paste_test!(an_empty_clipboard_is_reported_without_pasting, "empty");
paste_test!(non_png_data_never_reaches_the_prompt, "invalid");
paste_test!(oversized_images_are_rejected, "oversized");
paste_test!(a_stalled_bridge_has_a_bounded_deadline, "timeout");
paste_test!(an_unavailable_bridge_is_reported, "unavailable");
paste_test!(a_focus_change_cancels_the_pending_image, "focus-changed");
paste_test!(newly_typed_input_cancels_the_pending_image, "prompt-changed");
paste_test!(a_session_change_cancels_the_pending_image, "route-changed");
paste_test!(shell_mode_does_not_read_the_clipboard, "shell");
paste_test!(modal_dialogs_do_not_read_the_clipboard, "modal");
paste_test!(a_non_prompt_input_does_not_read_the_clipboard, "no-prompt");
paste_test!(unloading_cancels_a_pending_read_without_late_pastes, "unload-pending");
paste_test!(repeated_keys_share_one_pending_read, "repeat");
