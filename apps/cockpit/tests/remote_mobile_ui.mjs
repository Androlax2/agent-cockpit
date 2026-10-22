// The phone page's navigation, attention badge, agent formatting and reading position, each in a
// fresh page against fake DOM, history and server.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

class Element {
  constructor(tag = "div") { this.tagName = tag; this.children = []; this.attributes = {}; this.dataset = {}; this.style = {}; this.hidden = false; this.value = ""; this.scrollTop = 0; this.clientHeight = 100; this.scrollHeight = 500; }
  set textContent(value) { this.content = value; this.children = []; }
  get textContent() { return (this.content || "") + this.children.map(child => child.textContent).join(""); }
  append(...children) { for (const child of children) child.parent = this; this.children.push(...children); }
  replaceChildren(...children) { this.content = ""; this.children = []; this.append(...children); }
  getBoundingClientRect() { return { top: 0, bottom: 100 }; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name]; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  querySelectorAll(selector) {
    return this.children.flatMap(child => [
      ...(selector.startsWith("#") ? child.id === selector.slice(1) : selector.startsWith(".") ? (child.className || "").split(" ").includes(selector.slice(1)) : child.tagName === selector) ? [child] : [],
      ...child.querySelectorAll(selector),
    ]);
  }
  focus() {}
  addEventListener(name, handler) { this[`on${name}`] = handler; }
}

const settle = async () => { for (let index = 0; index < 20; index++) await Promise.resolve(); };

async function startPage({ sessions, approvals = [], messages = [], hash = "" }) {
  const roots = Object.fromEntries(["approvals", "list", "detail", "phone", "phone-content", "starter", "updated", "tabs", "work", "pairing", "view-title", "refresh-view"].map(id => [id, new Element()]));
  const document = {
    title: "", createElement: tag => new Element(tag),
    getElementById: id => roots[id] || Object.values(roots).map(root => root.querySelector(`#${id}`)).find(Boolean),
  };
  const listeners = {};
  const location = { hash, pathname: "/", search: "" };
  const hashOf = url => (url.includes("#") ? url.slice(url.indexOf("#")) : "");
  const entries = [{ state: null, hash }];
  let position = 0;
  const history = {
    get state() { return entries[position].state; },
    get length() { return entries.length; },
    get position() { return position; },
    pushState(state, _title, url) { entries.splice(position + 1); entries.push({ state, hash: hashOf(url) }); position++; location.hash = hashOf(url); },
    replaceState(state, _title, url) { entries[position] = { state, hash: hashOf(url) }; location.hash = hashOf(url); },
    back() { if (position === 0) return; position--; location.hash = entries[position].hash; listeners.popstate?.(); },
  };
  const server = { sessions, approvals, messages };
  const context = vm.createContext({
    document, console, URL, Map, Set, Promise, JSON, history, location, navigator: {}, setInterval: () => {},
    decodeURIComponent, encodeURIComponent,
    window: { navigator: {}, matchMedia: () => ({ matches: false }), addEventListener: (name, handler) => { listeners[name] = handler; } },
    fetch: async (path) => {
      const body = path === "/api/state" ? { sessions: server.sessions, approvals: server.approvals }
        : path.startsWith("/api/conversation/") ? { messages: server.messages, truncated: false }
        : path === "/api/projects" || path === "/api/agents" ? [] : { ok: true, message: "sent" };
      return { status: 200, ok: true, json: async () => structuredClone(body), text: async () => "" };
    },
  });
  vm.runInContext(readFileSync(new URL("../assets/remote/page.js", import.meta.url), "utf8"), context);
  await settle();
  const run = code => vm.runInContext(code, context);
  const button = (root, label) => root.querySelectorAll("button").find(node => node.textContent === label);
  return { roots, document, history, location, server, run, button };
}

const SESSIONS = [{ session: "s1", project: "Shop", agent: "claude", state: "waiting", features: ["dialogs"] }];

const tests = {
  async "the back gesture returns from a session to the list"() {
    const page = await startPage({ sessions: SESSIONS });
    page.roots.list.querySelector("button").onclick(); await settle();
    assert.equal(page.roots.detail.hidden, false);
    assert.equal(page.location.hash, "#session=s1");
    page.history.back(); await settle();
    assert.equal(page.run("opened"), null);
    assert.equal(page.roots.detail.hidden, true);
    assert.equal(page.roots.list.hidden, false);
  },

  async "the back button goes back in history instead of stacking a new entry"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openSession("s1")'); await settle();
    page.button(page.roots.detail, "‹ Sessions").onclick(); await settle();
    assert.equal(page.run("opened"), null);
    assert.equal(page.history.position, 0, "a later back gesture must leave the page, not reopen the session");
  },

  async "a notification link keeps the list beneath its session"() {
    const page = await startPage({ sessions: SESSIONS, hash: "#session=s1" });
    assert.equal(page.roots.detail.hidden, false);
    page.history.back(); await settle();
    assert.equal(page.roots.list.hidden, false);
    assert.equal(page.roots.detail.hidden, true);
  },

  async "a session that disappears drops its address"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openSession("s1")'); await settle();
    page.server.sessions = [];
    await page.run("refresh()"); await settle();
    assert.equal(page.location.hash, "");
  },

  async "the sessions tab counts each session that needs you once"() {
    const page = await startPage({
      sessions: [{ session: "s1", project: "Shop", state: "needs-you", features: [] },
                 { session: "s2", project: "Blog", state: "waiting", features: [] }],
      approvals: [{ id: "a1", session: "s1", project: "Shop", command: "make" },
                  { id: "a2", session: "s2", project: "Blog", command: "ls" }],
    });
    const tab = page.document.getElementById("tab-sessions");
    assert.equal(tab.querySelector(".badge").textContent, "2");
    assert.equal(tab.querySelector(".badge").hidden, false);
    assert.equal(tab.getAttribute("aria-label"), "Sessions, 2 need you");
    assert.equal(page.document.title, "(2) Claude sessions");
  },

  async "the badge disappears once nothing needs you"() {
    const page = await startPage({ sessions: [{ session: "s1", project: "Shop", state: "needs-you", features: [] }] });
    page.server.sessions = SESSIONS;
    await page.run("refresh()");
    const tab = page.document.getElementById("tab-sessions");
    assert.equal(tab.querySelector(".badge").hidden, true);
    assert.equal(tab.getAttribute("aria-label"), "Sessions");
    assert.equal(page.document.title, "Claude sessions");
  },

  async "agent Markdown becomes formatting without markup"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "a1", role: "assistant",
      text: "## Plan\nUse **bold** and `make test`.\n\n- first\n- second\n\n3. third\n\n```\n<img src=x onerror=alert(1)>\n```" }] });
    page.run('openSession("s1")'); await settle();
    const conversation = page.document.getElementById("conversation");
    assert.equal(conversation.querySelector(".message-heading").textContent, "Plan");
    assert.equal(conversation.querySelector("strong").textContent, "bold");
    assert.equal(conversation.querySelector("code").textContent, "make test");
    assert.deepEqual(conversation.querySelector("ul").children.map(item => item.textContent), ["first", "second"]);
    assert.equal(conversation.querySelector("ol").start, 3);
    assert.equal(conversation.querySelector("pre").textContent, "<img src=x onerror=alert(1)>");
    assert.equal(conversation.querySelectorAll("img").length, 0);
  },

  async "your own messages stay literal"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "u1", role: "user", text: "Keep **stars** and `ticks`" }] });
    page.run('openSession("s1")'); await settle();
    const conversation = page.document.getElementById("conversation");
    assert.equal(conversation.querySelectorAll("strong").length, 0);
    assert.match(conversation.textContent, /\*\*stars\*\* and `ticks`/);
  },

  async "new messages while reading back show a jump to the latest"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "a1", role: "assistant", text: "First" }] });
    page.run('openSession("s1")'); await settle();
    const conversation = page.document.getElementById("conversation");
    const jump = page.document.getElementById("jump-latest");
    assert.equal(jump.hidden, true);
    conversation.scrollTop = 0;
    page.server.messages = [...page.server.messages, { id: "a2", role: "assistant", text: "Second" }];
    await page.run("loadConversation()");
    assert.equal(jump.hidden, false);
    assert.equal(jump.textContent, "↓ New messages");
    jump.onclick();
    assert.equal(conversation.scrollTop, conversation.scrollHeight);
    assert.equal(jump.hidden, true);
  },

  async "session options stay closed until More is tapped"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openSession("s1")'); await settle();
    const panel = page.document.getElementById("session-more");
    const more = page.document.getElementById("session-more-toggle");
    assert.equal(panel.hidden, true);
    more.onclick();
    assert.equal(panel.hidden, false);
    assert.equal(more.getAttribute("aria-expanded"), "true");
    assert.equal(more.textContent, "Close");
  },
};

for (const [name, test] of Object.entries(tests)) {
  try { await test(); } catch (error) { error.message = `${name}: ${error.message}`; throw error; }
}
console.log(`mobile page: ${Object.keys(tests).length} navigation, badge, formatting and reading checks passed`);
