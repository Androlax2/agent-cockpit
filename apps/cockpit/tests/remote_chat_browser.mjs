// Optional real-browser companion to the dependency-free default Node/Cargo tests.
// REMOTE_CHAT_BROWSER names an already-installed Chromium; nothing is downloaded.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";

const executable = process.env.REMOTE_CHAT_BROWSER;
assert.ok(executable, "Set REMOTE_CHAT_BROWSER to an installed Chromium executable");
const artifacts = process.env.REMOTE_CHAT_ARTIFACTS || "/tmp/opencode/mobile-chat";
mkdirSync(artifacts, { recursive: true });
const profile = mkdtempSync(join(artifacts, "browser-"));
const session = { session: "s1", project: "Mobile test", agent: "opencode", state: "needs-you", needs_input: true, features: [] };
let history = { messages: [{ id: "user", role: "user", text: "Make the mobile cockpit simple." },
  { id: "agent", role: "assistant", text: "Ready for your reply.\n\n<img src=x onerror=alert(1)> stays plain text." }], truncated: false };
let approvals = [];
const posts = [];
const server = createServer((request, response) => {
  const path = request.url;
  if (path === "/" || path === "/page.js") {
    response.setHeader("Content-Type", path === "/" ? "text/html" : "text/javascript");
    response.end(readFileSync(new URL(path === "/" ? "../assets/remote/page.html" : "../assets/remote/page.js", import.meta.url))); return;
  }
  response.setHeader("Content-Type", "application/json");
  if (request.method === "POST") {
    let text = ""; request.on("data", chunk => { text += chunk; });
    request.on("end", () => { posts.push({ path, body: JSON.parse(text) }); response.end(JSON.stringify({ ok: true, message: "sent" })); }); return;
  }
  const data = path === "/api/state" ? { sessions: [session], approvals } : path.startsWith("/api/conversation/") ? history : [];
  response.end(JSON.stringify(data));
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const browser = spawn(executable, ["--headless", "--no-sandbox", "--disable-gpu", "--remote-debugging-pipe", `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
let stderr = ""; browser.stderr.on("data", chunk => { stderr += chunk; });
let sequence = 0;
const waiting = new Map();
let buffered = "";
browser.stdio[4].on("data", chunk => {
  buffered += chunk.toString();
  let end;
  while ((end = buffered.indexOf("\0")) >= 0) {
    const message = JSON.parse(buffered.slice(0, end)); buffered = buffered.slice(end + 1);
    const pending = waiting.get(message.id);
    if (pending) { waiting.delete(message.id); clearTimeout(pending.timer); message.error ? pending.reject(new Error(JSON.stringify(message.error))) : pending.resolve(message.result); }
  }
});
function command(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { waiting.delete(id); reject(new Error(`Timed out: ${method}\n${stderr}`)); }, 15000);
    waiting.set(id, { resolve, reject, timer });
    browser.stdio[3].write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + "\0");
  });
}
try {
  const { targetId } = await command("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await command("Target.attachToTarget", { targetId, flatten: true });
  const cdp = (method, params) => command(method, params, sessionId);
  const evaluate = async expression => {
    const result = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await cdp("Page.enable");
  await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await cdp("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/` });
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await evaluate('document.querySelector(".session-card") !== null')) break;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  await evaluate('document.querySelector(".session-card").click()');
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await evaluate('document.querySelector(".message-text") !== null')) break;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.equal(await evaluate('document.querySelectorAll("#conversation img").length'), 0);
  assert.equal(await evaluate('document.querySelector("#terminal-details").open'), false);
  assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
  await evaluate('const reply = document.querySelector("#chat-reply"); reply.value = "Use option A"; reply.dispatchEvent(new Event("input")); document.querySelector("#chat-send").click()');
  for (let attempt = 0; attempt < 100 && !posts.length; attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(posts[0], { path: "/api/message/s1", body: { text: "Use option A" } });
  await cdp("Emulation.setDeviceMetricsOverride", { width: 320, height: 400, deviceScaleFactor: 1, mobile: true });
  await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
  const replyBounds = await evaluate('JSON.stringify(document.querySelector("#chat-send").getBoundingClientRect().toJSON())');
  assert.ok(JSON.parse(replyBounds).bottom <= 400, `Reply action hidden by keyboard-size viewport: ${replyBounds}`);
  approvals = [{ id: "0123456789ab", session: "s1", project: "Mobile test", cwd: "/project", command: "echo checked", why: "Check the requested change" }];
  await evaluate("refresh()");
  const clickRun = 'Array.from(document.querySelectorAll("#chat-actions button")).find(button => button.textContent === "Run").click()';
  await evaluate(clickRun);
  assert.equal(posts.filter(post => post.path.startsWith("/api/decide/")).length, 0);
  await evaluate('Array.from(document.querySelectorAll("#chat-actions button")).find(button => button.textContent === "Tap again to run").click()');
  for (let attempt = 0; attempt < 100 && posts.length < 2; attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(posts[1].body, { run: true, command: "echo checked" });
  approvals = [];
  history = { messages: Array.from({ length: 30 }, (_, index) => ({ id: `${index}`, role: index % 2 ? "assistant" : "user", text: `Message ${index}: ${"long text ".repeat(20)}` })), truncated: false };
  await evaluate('refresh().then(loadConversation)');
  await evaluate('document.querySelector("#conversation").scrollTop = 25; document.querySelector("#chat-reply").focus()');
  await evaluate("refresh().then(loadConversation)");
  assert.equal(await evaluate('document.querySelector("#conversation").scrollTop'), 25);
  assert.equal(await evaluate('document.activeElement.id'), "chat-reply");
  await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const screenshot = await cdp("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(artifacts, "mobile-chat.png"), Buffer.from(screenshot.data, "base64"));
  console.log("real Chromium: 390px/320px layouts, safe text, reply, approval confirmation, focus and reading position passed");
} finally {
  browser.kill();
  server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  await new Promise(resolve => browser.once("exit", resolve));
  rmSync(profile, { recursive: true, force: true });
}
