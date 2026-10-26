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
let inputs = [];
let isPaired = true;
const otherSession = { ...session, session: "s2", project: "Other session" };
let chief = { status: "Active", today: { answered: 1, escalated: 1 }, counts: [], answers: [], habits: [], escalations: [] };
const inputOutput = "1 commit(s) for /home/theo/agent-cockpit, the running copy of agent-cockpit the host runs:\n  [PATCH] chore(cockpit): drop the claude-sessions name\n\n apps/cockpit/tests/cockpit_script.rs  | 34 ----------\n config/bin/.local/bin/claude-sessions |  7 ---\n 3 files changed, 61 deletions(-)\n";
const posts = [];
const server = createServer((request, response) => {
  const path = request.url;
  if (path === "/tokens.css") {
    response.setHeader("Content-Type", "text/css");
    response.end(readFileSync(new URL("../../../design/tokens.cockpit-ui.css", import.meta.url))); return;
  }
  if (path === "/" || path === "/page.js") {
    response.setHeader("Content-Type", path === "/" ? "text/html" : "text/javascript");
    response.end(readFileSync(new URL(path === "/" ? "../assets/remote/page.html" : "../assets/remote/page.js", import.meta.url))); return;
  }
  response.setHeader("Content-Type", "application/json");
  if (path === "/api/state" && !isPaired) { response.statusCode = 401; response.end("Pair this phone again"); return; }
  if (request.method === "POST") {
    let text = ""; request.on("data", chunk => { text += chunk; });
    request.on("end", () => { posts.push({ path, body: JSON.parse(text) }); response.end(JSON.stringify({ ok: true, message: "sent" })); }); return;
  }
  if (path.startsWith("/api/input/")) { response.setHeader("Content-Type", "text/plain"); response.end(inputOutput); return; }
  const data = path === "/api/state" ? { sessions: [session, otherSession], approvals, inputs }
    : path === "/api/chief" ? chief : path.startsWith("/api/conversation/") ? history
    : path === "/api/usage" ? { windows: [], cost: { today: 0, week: 0, month: 0 } }
    : path === "/api/pulls" ? { pulls: [] } : [];
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
  await evaluate('const reading = document.querySelector("#conversation"); reading.scrollTop = reading.children[10].offsetTop - reading.offsetTop + 20');
  const anchorBefore = await evaluate('document.querySelector("#conversation").children[10].getBoundingClientRect().top');
  history = { messages: history.messages.slice(5).concat([{ id: "new", role: "assistant", text: "New reply" }]), truncated: true };
  await evaluate("loadConversation()");
  const anchorAfter = await evaluate('Array.from(document.querySelector("#conversation").children).find(turn => turn.textContent.includes("Message 10:")).getBoundingClientRect().top');
  assert.ok(Math.abs(anchorAfter - anchorBefore) < 2, `Rolling history moved the visible message: ${anchorBefore} -> ${anchorAfter}`);
  await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const frame = () => evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const capture = async name => writeFileSync(join(artifacts, name), Buffer.from((await cdp("Page.captureScreenshot", { format: "png" })).data, "base64"));
  await capture("mobile-chat.png");
  assert.equal(await evaluate('getComputedStyle(document.querySelector("#tabs")).display'), "none", "the chat hides the tab bar");
  history = { messages: [{ id: "u", role: "user", text: "How is the fix going?" },
    { id: "m", role: "assistant", text: "## Done\nThe cart now keeps **its items** after `make test`.\n\n- Fixed the session cookie\n- Added a regression test\n\n```\nmake test  # 42 passed\n```" }], truncated: false };
  await evaluate("loadConversation()"); await frame();
  assert.equal(await evaluate('document.querySelector("#conversation strong")?.textContent'), "its items");
  assert.equal(await evaluate('document.querySelectorAll("#conversation li").length'), 2);
  await capture("mobile-chat-formatted.png");
  const sendBottom = () => evaluate('document.querySelector("#chat-send").getBoundingClientRect().bottom');
  await evaluate('const grow = document.querySelector("#chat-reply"); grow.value = "one\\ntwo\\nthree\\nfour\\nfive"; grow.dispatchEvent(new Event("input"))'); await frame();
  assert.ok(await evaluate('document.querySelector("#chat-reply").getBoundingClientRect().height') > 90, "the reply box grows with its text");
  assert.ok(await sendBottom() <= 844, "a grown reply box keeps Send on screen");
  await evaluate('document.querySelector("#session-more-toggle").click()'); await frame();
  assert.equal(await evaluate('document.querySelector("#session-more").hidden'), false);
  assert.ok(await sendBottom() <= 844, "the open options keep Send on screen");
  await capture("mobile-chat-more.png");
  await evaluate("history.back()");
  for (let attempt = 0; attempt < 100 && !(await evaluate('document.querySelector("#detail").hidden')); attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(await evaluate('document.querySelector("#detail").hidden'), true, "back returns to the list");
  assert.equal(await evaluate('getComputedStyle(document.querySelector("#tabs")).display'), "flex");
  await capture("mobile-list.png");
  inputs = [{ id: "0123456789ab", command: "cockpit apply /home/theo/IdeaProjects/agent-cockpit", prompt: "Apply these 1 commit(s) to /home/theo/agent-cockpit? [y/N]" }];
  await evaluate("refresh()");
  for (let attempt = 0; attempt < 100 && !(await evaluate('document.querySelector(".input-output")?.textContent.includes("drop the claude-sessions")')); attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(await evaluate('document.querySelector(".question").textContent'), "Apply these 1 commit(s) to /home/theo/agent-cockpit? [y/N]");
  assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true, "a long question must not widen the page");
  await capture("mobile-command-question.png");
  inputs = [];
  chief.escalations = [{ id: "decision-1", revision: 4, session_id: "s1", category: "security", question: "Should I rotate the greyproxy API token now?",
    recommendation: "Security questions always come to you.", suggested_answer: "Yes, rotate it and update the dashboard config." }];
  chief.counts = [{ session_id: "s1", count: 1 }];
  chief.answers = [{ id: "answer-1", session_id: "s1", project: "path:/homelab", question: "Which DNS fix should I keep?", answer: "Keep the Unbound one.",
    source: "chief", reason: "Smallest fix", delivery: "received", overrule: null, habits: [], created_at_ms: Date.now() }];
  await evaluate('refresh().then(() => openSession("s1"))');
  assert.equal(await evaluate('document.querySelector(".chief-escalation .question").textContent'), "! Chief needs you · Security");
  for (const width of [390, 320]) {
    await cdp("Emulation.setDeviceMetricsOverride", { width, height: 844, deviceScaleFactor: 1, mobile: true });
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true, "Chief text must wrap without widening the phone");
    assert.equal(await evaluate('document.querySelector(".chief-use").getBoundingClientRect().top >= document.querySelector(".chief-escalation").getBoundingClientRect().bottom'), true, "escalation actions sit below the bordered explanation");
    assert.equal(await evaluate('document.querySelector(".chief-log").getBoundingClientRect().bottom <= document.querySelector("#session-chief").getBoundingClientRect().bottom'), true, "the answer log stays visible below a normal escalation");
    await capture(`mobile-chief-escalation-${width}.png`);
  }
  await evaluate('document.querySelector(".chief-log").click()');
  assert.equal(await evaluate('document.activeElement.textContent'), "‹ Back");
  assert.equal(await evaluate('document.querySelector("#detail").inert'), true);
  assert.equal(await evaluate('document.querySelector(".chief-answer").getBoundingClientRect().top - document.querySelector(".chief-context").getBoundingClientRect().bottom < 32'), true, "empty status messages must not add a blank section above the answers");
  await capture("mobile-chief-answers.png");
  await evaluate('document.querySelector(".chief-overrule").click()');
  assert.equal(await evaluate('document.activeElement.className'), "chief-input");
  await capture("mobile-chief-overrule.png");
  await evaluate('history.back()');
  for (let attempt = 0; attempt < 100 && !(await evaluate('document.querySelector("#chief").hidden')); attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(await evaluate('document.activeElement.className'), "chief-log");
  chief.habits = [{ id: "habit-1", text: "Keep boot-time host overrides for the NAS", project: "path:/homelab", state: "active", created_at_ms: Date.now() },
    { id: "habit-2", text: "Run tests before applying", project: null, state: "active", created_at_ms: 1 }];
  await evaluate('refresh().then(() => openChief("habits"))');
  assert.equal(await evaluate('document.querySelectorAll(".chief-habit").length'), 2);
  for (const width of [390, 320]) {
    await cdp("Emulation.setDeviceMetricsOverride", { width, height: 844, deviceScaleFactor: 1, mobile: true });
    assert.equal(await evaluate('document.querySelector("#chief").scrollWidth <= innerWidth'), true, "habit text wraps at phone widths");
    assert.equal(await evaluate('Array.from(document.querySelectorAll(".chief-habit button")).every(button => button.getBoundingClientRect().height >= 44)'), true, "habit actions retain touch targets");
    await capture(`mobile-chief-habits-${width}.png`);
  }
  await evaluate('history.replaceState(null, "", "/"); closeSession(); showTab("status")');
  for (let attempt = 0; attempt < 100 && !(await evaluate('document.querySelector("#chief-status button") !== null')); attempt++) await new Promise(resolve => setTimeout(resolve, 20));
  for (const label of ["Habits", "Answer log"]) {
    await evaluate(`Array.from(document.querySelectorAll("#chief-status button")).find(button => button.textContent === "${label}").click()`);
    await evaluate('history.back()');
    for (let attempt = 0; attempt < 100 && !(await evaluate('document.querySelector("#chief").hidden')); attempt++) await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(await evaluate('document.activeElement.textContent'), label, "Back restores the Status opener, not a hidden session control");
  }
  for (const target of ["s1", "s2"]) {
    await evaluate('openSession("s1"); openChief("answers", "s1")');
    await evaluate(`navigator.serviceWorker.dispatchEvent(new MessageEvent("message", { data: { session: "${target}" } }))`);
    assert.equal(await evaluate('document.querySelector("#chief").hidden'), true, "notification target is not covered by Chief");
    assert.equal(await evaluate('document.querySelector("#detail").inert'), false);
    assert.equal(await evaluate('opened'), target);
  }
  await evaluate('openChief("habits"); openChief("answers")');
  isPaired = false;
  await evaluate('refresh()');
  assert.equal(await evaluate('document.querySelector("#chief").hidden'), true, "expired pairing closes Chief");
  assert.equal(await evaluate('document.querySelector("#pairing").hidden'), false);
  assert.equal(await evaluate('document.querySelector("#tabs").inert'), false);
  await evaluate('new Promise(resolve => { window.addEventListener("popstate", resolve, { once: true }); history.back(); })');
  assert.equal(await evaluate('document.querySelector("#chief").hidden'), true, "history cannot reopen Chief while unpaired");
  await capture("mobile-chief-expired-pairing.png");
  console.log("real Chromium: 390px/320px layouts, safe text, reply, approval confirmation, focus, reading position, formatting, growing reply, options panel, back gesture, Chief notification navigation and expired pairing passed");
} finally {
  browser.kill();
  server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  await new Promise(resolve => browser.once("exit", resolve));
  rmSync(profile, { recursive: true, force: true });
}
