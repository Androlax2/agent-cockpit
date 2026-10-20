import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

class Element {
  constructor(tag = "div") { this.tagName = tag; this.children = []; this.attributes = {}; this.dataset = {}; this.style = {}; this.hidden = false; this.value = ""; this.scrollTop = 0; this.clientHeight = 100; this.scrollHeight = 500; }
  set textContent(value) { this.content = value; this.children = []; }
  get textContent() { return (this.content || "") + this.children.map(child => child.textContent).join(""); }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.content = ""; this.children = children; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name]; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  querySelectorAll(selector) {
    return this.children.flatMap(child => [
      ...(selector.startsWith("#") ? child.id === selector.slice(1) : selector.startsWith(".") ? (child.className || "").split(" ").includes(selector.slice(1)) : child.tagName === selector) ? [child] : [],
      ...child.querySelectorAll(selector),
    ]);
  }
  focus() { document.activeElement = this; }
  addEventListener(name, handler) { this[`on${name}`] = handler; }
}
const roots = Object.fromEntries(["approvals", "list", "detail", "phone", "starter", "updated", "tabs", "work", "pairing"].map(id => [id, new Element()]));
const document = {
  createElement: tag => new Element(tag), activeElement: null,
  getElementById: id => roots[id] || Object.values(roots).map(root => root.querySelector(`#${id}`)).find(Boolean),
};
const requests = [];
let current = { sessions: [
  { session: "s1", project: "First", agent: "opencode", state: "needs-you", needs_input: true, features: [] },
  { session: "s2", project: "Second", agent: "claude", state: "waiting", features: ["dialogs"] },
], approvals: [
  { id: "approve1", session: "s1", project: "First", command: "echo safe", why: "Check the change" },
  { id: "approve2", session: "s2", project: "Second", command: "echo other" },
] };
let delayHistory = null;
let failSend = false;
let holdSend = null;
const history = { messages: [{ id: "u1", role: "user", text: "Hello" }, { id: "a1", role: "assistant", text: "<img src=x onerror=alert(1)>Which option?" }], truncated: false };
const context = vm.createContext({
  document, console, URL, Map, Set, Promise, JSON,
  window: { navigator: {}, matchMedia: () => ({ matches: false }) }, navigator: {}, location: { hash: "" },
  setInterval: () => {},
  fetch: async (path, options = {}) => {
    requests.push({ path, options });
    if (path.startsWith("/api/conversation/") && delayHistory) await delayHistory;
    if (path.startsWith("/api/message/") && holdSend) await holdSend;
    if (path.startsWith("/api/message/") && failSend) throw new Error("offline");
    const body = path === "/api/state" ? current : path === "/api/conversation/s2" ? { messages: [{ id: "second", role: "user", text: "Second conversation" }], truncated: false } : path.startsWith("/api/conversation/") ? history : path === "/api/projects" || path === "/api/agents" ? [] : { ok: true, message: "sent" };
    return { status: 200, ok: true, json: async () => structuredClone(body), text: async () => "terminal text" };
  },
});
vm.runInContext(readFileSync(new URL("../assets/remote/page.js", import.meta.url), "utf8"), context);
const run = code => vm.runInContext(code, context);
const settle = async () => { for (let index = 0; index < 12; index++) await Promise.resolve(); };
const button = (root, label) => root.querySelectorAll("button").find(node => node.textContent === label);
await settle();
run('openSession("s1")'); await settle();
assert.match(roots.detail.textContent, /Hello/);
assert.match(roots.detail.textContent, /Which option/);
assert.equal(roots.detail.querySelectorAll("img").length, 0, "messages must never become markup");
assert.equal(roots.detail.querySelectorAll("details").some(node => node.querySelector("summary")?.textContent.includes("Terminal")), true);
assert.equal(requests.some(request => request.path.startsWith("/api/screen/")), false, "terminal must not load until requested");
const terminal = document.getElementById("terminal-details"); terminal.open = true;
await run("loadScreen()");
assert.equal(document.getElementById("screen").textContent, "terminal text");
assert.equal(roots.detail.textContent.includes("echo other"), false, "no other session approval in chat");
assert.equal(button(roots.detail, "Yes"), undefined, "chat questions are not permission dialogs");

let input = roots.detail.querySelector("textarea");
input.value = "Draft one"; input.oninput(); input.focus();
const conversation = document.getElementById("conversation"); conversation.scrollTop = 25;
current.sessions[0].state = "working";
await run("refresh()"); await settle();
assert.equal(roots.detail.querySelector("textarea"), input, "polling must preserve input node and focus");
assert.equal(document.activeElement, input);
assert.equal(conversation.scrollTop, 25);
run('openSession("s2")'); await settle();
assert.equal(roots.detail.querySelector("textarea").value, "", "drafts must not leak between sessions");
run('openSession("s1")'); await settle();
assert.equal(roots.detail.querySelector("textarea").value, "Draft one");

let releaseHistory;
delayHistory = new Promise(resolve => { releaseHistory = resolve; });
const pending = run("loadConversation()");
run('openSession("s2")');
delayHistory = null; await settle();
releaseHistory(); await pending; await settle();
assert.equal(run("opened"), "s2");
assert.match(roots.detail.textContent, /Second/);
assert.equal(document.getElementById("conversation").textContent.includes("Which option?"), false, "late first-session text must not replace the second session");

run('openSession("s1")'); await settle();
const runApproval = button(roots.detail, "Run");
runApproval.onclick();
assert.equal(requests.filter(request => request.path.includes("/api/decide/")).length, 0);
await button(roots.detail, "Tap again to run").onclick();
const decision = requests.find(request => request.path === "/api/decide/approve1");
assert.deepEqual(JSON.parse(decision.options.body), { run: true, command: "echo safe" });
const sentCount = () => requests.filter(request => request.path.startsWith("/api/message/")).length;
input = roots.detail.querySelector("textarea"); input.value = "Keep my reply"; input.oninput();
failSend = true;
await button(roots.detail, "Queue").onclick();
assert.equal(input.value, "Keep my reply");
assert.match(roots.detail.textContent, /offline/i);
failSend = false;
let releaseSend;
holdSend = new Promise(resolve => { releaseSend = resolve; });
const before = sentCount();
const sending = button(roots.detail, "Queue").onclick();
button(roots.detail, "Sending…").onclick();
await settle(); assert.equal(sentCount(), before + 1, "double taps must not submit twice");
input.value = "New draft while sending"; input.oninput();
releaseSend(); await sending; holdSend = null;
assert.equal(input.value, "New draft while sending", "successful send must not erase newer typing");
input.value = "Send before switching"; input.oninput();
holdSend = new Promise(resolve => { releaseSend = resolve; });
const switchedSend = button(roots.detail, "Queue").onclick();
run('openSession("s2")'); await settle();
run('openSession("s1")'); await settle();
releaseSend(); await switchedSend; holdSend = null;
assert.equal(roots.detail.querySelector("textarea").value, "", "a successful reply must clear its visible draft even after switching away and back");
current.approvals.push({ id: "decline1", session: "s1", project: "First", command: "echo declined" });
await run("refresh()");
await roots.detail.querySelectorAll("article").find(node => node.textContent.includes("echo declined")).querySelectorAll("button").find(node => node.textContent === "Decline").onclick();
const declined = requests.find(request => request.path === "/api/decide/decline1");
assert.deepEqual(JSON.parse(declined.options.body), { run: false, command: "echo declined" });
current.sessions[0].state = "needs-you"; current.sessions[0].needs_input = false;
await run("refresh()");
assert.equal(document.getElementById("chat-send").disabled, true, "real dialogs cannot take chat replies");
current.sessions[0].needs_input = true;
await run("refresh()");
assert.equal(document.getElementById("chat-send").disabled, false, "ordinary questions accept replies");
console.log("mobile chat: messages, safe text, scoped approvals, drafts, focus, scroll, stale responses, offline retry and duplicate submission passed");
