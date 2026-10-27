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
  removeChild(child) { this.children.splice(this.children.indexOf(child), 1); child.parent = null; }
  insertBefore(child, before) {
    if (child.parent) child.parent.removeChild(child);
    const index = before === null ? this.children.length : this.children.indexOf(before);
    this.children.splice(index, 0, child); child.parent = this;
  }
  getBoundingClientRect() { return { top: 0, bottom: 100 }; }
  getClientRects() {
    for (let node = this; node; node = node.parent) { if (node.hidden) return []; }
    return [this.getBoundingClientRect()];
  }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  removeAttribute(name) { delete this.attributes[name]; }
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

async function startPage({ sessions, approvals = [], inputs = [], messages = [], hash = "", inputOutput = "", chief = { status: "Off", answers: [], habits: [], escalations: [], counts: [] } }) {
  const roots = Object.fromEntries(["approvals", "list", "detail", "chief", "phone", "phone-content", "starter", "updated", "tabs", "work", "pairing", "view-title", "refresh-view"].map(id => [id, new Element()]));
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
  const server = { sessions, approvals, inputs, messages, chief, posts: [], requests: [] };
  const context = vm.createContext({
    document, console, URL, Map, Set, Promise, JSON, history, location, navigator: {}, setInterval: () => {},
    decodeURIComponent, encodeURIComponent,
    window: { navigator: {}, matchMedia: () => ({ matches: false }), addEventListener: (name, handler) => { listeners[name] = handler; } },
    fetch: async (path, options = {}) => {
      server.requests.push({ path, method: options.method });
      if (path === "/api/state" && server.unpaired) return { status: 401, ok: false, text: async () => "Pair this phone again" };
      if (options.method === "POST") server.posts.push({ path, body: options.body === undefined ? undefined : JSON.parse(options.body) });
      if (path === "/api/chief" && options.method === "POST" && server.chiefPost) return server.chiefPost();
      if (path === "/api/chief" && options.method !== "POST" && server.chiefFetch) return server.chiefFetch();
      if (path === "/api/chief" && options.method !== "POST") return { status: server.chiefError ? 503 : 200, ok: !server.chiefError, json: async () => structuredClone(server.chief), text: async () => "Chief host offline" };
      if (path.startsWith("/api/input/") && options.method !== "POST") return { status: 200, ok: true, text: async () => inputOutput };
      const body = path === "/api/state" ? { sessions: server.sessions, approvals: server.approvals, inputs: server.inputs, counts: server.counts }
        : path.startsWith("/api/conversation/") ? { messages: server.messages, truncated: false }
        : path === "/api/usage" ? { windows: [], cost: { today: 0, week: 0, month: 0 } }
        : path === "/api/pulls" ? { pulls: [] }
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

function inboxChief({ steps = [{ text: "Close your Stadeo guide" }, { text: "Tell the agent to go on", message: "Go on: restart osd and run your check." }] } = {}) {
  return { status: "Active", today: { answered: 5, escalated: 1 }, counts: [], answers: [], habits: [],
    session_names: { s1: "Persist Stadeo team · agent-cockpit" },
    escalations: [{ id: "d1", revision: 3, state: "open", session_id: "s1", category: "not_sure", question: "Restart osd?",
      recommendation: "The fix is in, but osd won't restart while your guide is open.", suggested_answer: "Go on.", later_until_ms: null,
      created_at_ms: Date.now() - 240000, situation: null, steps }] };
}

const INPUT = { id: "0123456789ab", command: "cockpit apply /clone", prompt: "Apply these 1 commit(s)? [y/N]" };
const SESSIONS = [{ session: "s1", project: "Shop", agent: "claude", state: "waiting", features: ["dialogs"] }];

const ASLEEP = [{ session: "s1", project: "Shop", agent: "claude", state: "asleep", since: Math.floor(Date.now() / 1000) - 7200,
                  features: ["dialogs", "interrupt", "remote_control"] }];

const tests = {
  async "expired pairing closes Chief and history cannot reopen cached actions"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openChief("habits"); openChief("answers")');
    page.server.unpaired = true;
    await page.run("refresh()");
    assert.equal(page.roots.pairing.hidden, false);
    assert.equal(page.roots.chief.hidden, true);
    assert.equal(page.roots.detail.inert, false);
    page.history.back();
    assert.equal(page.roots.chief.hidden, true);
    page.run('openChief("habits")');
    assert.equal(page.roots.chief.hidden, true);
  },

  async "notification navigation closes Chief for the same or another session"() {
    for (const target of ["s1", "s2"]) {
      const page = await startPage({ sessions: [...SESSIONS, { ...SESSIONS[0], session: "s2", project: "Other" }] });
      page.run('openSession("s1"); openChief("answers", "s1")');
      page.run(`openSession("${target}")`);
      assert.equal(page.roots.chief.hidden, true);
      assert.equal(page.roots.detail.inert, false);
      assert.equal(page.run("opened"), target);
      assert.equal(page.location.hash, `#session=${target}`);
    }
  },

  async "Chief Status updates while open and shows host loss and recovery"() {
    const page = await startPage({ sessions: SESSIONS, chief: { status: "Active", today: { answered: 5, escalated: 1 }, counts: [], answers: [], habits: [], escalations: [] } });
    page.run('showTab("status")'); await settle();
    assert.ok(page.roots.work.textContent.includes("answered 5 today"));
    page.server.chiefError = true; await page.run("refresh()"); await settle();
    assert.ok(page.roots.work.textContent.includes("Chief unavailable"));
    assert.ok(!page.roots.work.textContent.includes("answered 5 today"));
    page.server.chiefError = false; page.server.chief.today.answered = 0;
    await page.run("refresh()"); await settle();
    assert.ok(page.roots.work.textContent.includes("answered 0 today"));
  },

  async "an older Chief response cannot overwrite a newer unavailable state"() {
    const page = await startPage({ sessions: SESSIONS });
    const reads = [];
    page.server.chiefFetch = () => new Promise(resolve => reads.push(resolve));
    const oldRead = page.run("refreshChief()"); const newRead = page.run("refreshChief()");
    reads[1]({ ok: false, status: 503, text: async () => "Host lost" }); await newRead;
    reads[0]({ ok: true, status: 200, json: async () => ({ status: "Active", answers: [], habits: [], escalations: [] }) }); await oldRead;
    assert.equal(page.run("chiefState"), null);
    assert.ok(page.run("chiefError").includes("Host lost"));
  },

  async "unrelated Chief answers preserve the correction editor node and draft"() {
    const record = { id: "a1", session_id: "s1", project: "path:/Shop", question: "Which?", answer: "B", reason: "Smallest", source: "chief", delivery: "received", overrule: null, habits: [], created_at_ms: 1 };
    const page = await startPage({ sessions: SESSIONS, chief: { status: "Active", counts: [], escalations: [], habits: [], answers: [record] } });
    page.run('openChief("answers")'); page.button(page.roots.chief, "Overrule").onclick();
    const input = page.roots.chief.querySelector(".chief-input"); input.value = "Keep typing"; input.oninput();
    page.server.chief.answers.unshift({ ...record, id: "a2", session_id: "s2" });
    await page.run("refresh()"); await settle();
    assert.equal(page.roots.chief.querySelector(".chief-input"), input);
    assert.equal(input.value, "Keep typing");
  },

  async "global Chief answers identify the recipient before and during correction"() {
    const record = { id: "a1", session_id: "s1", project: "path:/Shop", question: "Which?", answer: "B", reason: "Smallest", source: "chief", delivery: "received", overrule: null, habits: [], created_at_ms: 1 };
    const page = await startPage({ sessions: SESSIONS, chief: { status: "Active", session_names: { s1: "Checkout developer · Shop" }, counts: [], escalations: [], habits: [], answers: [record] } });
    page.run('openChief("answers")');
    assert.ok(page.roots.chief.querySelector(".chief-answer").textContent.includes("Checkout developer · Shop"));
    page.button(page.roots.chief, "Overrule").onclick();
    assert.ok(page.roots.chief.querySelector(".chief-editor").textContent.includes("Checkout developer · Shop"));
  },

  async "a definitive Chief rejection allows corrected input but uncertain delivery does not"() {
    for (const uncertain of [false, true]) {
      const chief = { status: "Active", counts: [], answers: [], habits: [], escalations: [{ id: "d1", revision: 4, session_id: "s1", category: "scope", question: "Continue?", suggested_answer: null }] };
      const page = await startPage({ sessions: SESSIONS, chief });
      page.server.chiefPost = async () => ({ ok: false, status: 409, json: async () => ({ ok: false, message: "Rejected", outcome_uncertain: uncertain }) });
      page.run('openSession("s1")'); page.button(page.roots.detail, "Write my own answer").onclick(); await settle();
      const input = page.roots.chief.querySelector(".chief-input"); input.value = "Bad\ttext"; input.oninput();
      await page.button(page.roots.chief, "Send").onclick(); await page.button(page.roots.chief, "Tap again to send").onclick(); await settle();
      const corrected = page.roots.chief.querySelector(".chief-input"); corrected.value = "Corrected text"; corrected.oninput();
      assert.equal(page.button(page.roots.chief, "Send").disabled, uncertain);
    }
  },
  async "Chief inbox opens from Status with the question card"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.run('showTab("status")'); await settle();
    page.button(page.roots.work, "Needs you · 1").onclick(); await settle();
    const text = page.roots.chief.textContent;
    for (const expected of ["! Chief needs you · 1 of 1", "agent-cockpit", "Persist Stadeo team", "Not sure · asked 4m ago", "What's going on",
      "osd won't restart", "◆ Chief suggests · 2 steps", "Close your Stadeo guide", "Written by the Chief: check it before acting."]) {
      assert.ok(text.includes(expected), `${expected}: ${text}`);
    }
    for (const label of ["Do this, step by step", "Write my own", "Later", "Full details ›"]) assert.ok(page.button(page.roots.chief, label), label);
  },

  async "Chief inbox steps end with a second-tap send of the exact message, once"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.run('openInbox()'); await settle();
    page.button(page.roots.chief, "Do this, step by step").onclick(); await settle();
    assert.ok(page.roots.chief.textContent.includes("Step 1 of 2"));
    page.button(page.roots.chief, "Done, next step").onclick(); await settle();
    assert.ok(page.roots.chief.textContent.includes("Step 2 of 2 · Send to the agent"));
    assert.ok(page.roots.chief.textContent.includes("Go on: restart osd and run your check."));
    await page.button(page.roots.chief, "Send").onclick(); await settle();
    assert.equal(page.server.posts.length, 0, "the first tap only arms");
    const again = page.button(page.roots.chief, "Tap again to send");
    await Promise.all([again.onclick(), again.onclick()]); await settle();
    const posts = page.server.posts.filter(post => post.path === "/api/chief");
    assert.equal(posts.length, 1);
    assert.deepEqual([posts[0].body.action.action, posts[0].body.action.answer, posts[0].body.action.expected_revision], ["answer_decision", "Go on: restart osd and run your check.", 3]);
    assert.deepEqual(posts[0].body.expected, page.server.chief.escalations[0]);
    assert.ok(page.roots.chief.textContent.includes("✓ Sent to agent-cockpit › Persist Stadeo team"));
    assert.ok(page.button(page.roots.chief, "Stop here"));
  },

  async "Chief inbox edit first prefills and Back keeps the message"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief({ steps: null }) });
    page.run('openInbox()'); await settle();
    page.button(page.roots.chief, "Do this, step by step").onclick(); await settle();
    page.button(page.roots.chief, "Edit first").onclick(); await settle();
    const input = page.roots.chief.querySelector(".chief-input");
    assert.equal(input.value, "Go on.");
    input.value = "Changed"; input.oninput();
    page.button(page.roots.chief, "Back").onclick(); await settle();
    assert.ok(page.roots.chief.textContent.includes("Go on."));
    assert.ok(!page.roots.chief.textContent.includes("Changed"));
  },

  async "Chief inbox write my own needs a second tap and Later puts it off"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.run('openInbox()'); await settle();
    page.button(page.roots.chief, "Write my own").onclick(); await settle();
    const input = page.roots.chief.querySelector(".chief-input"); input.value = "I'll do it"; input.oninput();
    await page.button(page.roots.chief, "Send").onclick();
    assert.equal(page.server.posts.length, 0);
    page.button(page.roots.chief, "Back").onclick(); await settle();
    await page.button(page.roots.chief, "Later").onclick(); await settle();
    const posts = page.server.posts.filter(post => post.path === "/api/chief");
    assert.deepEqual([posts[0].body.action.action, posts[0].body.action.decision, posts[0].body.action.expected_revision], ["later_decision", "d1", 3]);
  },

  async "a deferred Chief question is not counted until it comes back"() {
    const chief = inboxChief();
    chief.escalations.push({ ...chief.escalations[0], id: "d2", later_until_ms: Date.now() + 600000 });
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('showTab("status")'); await settle();
    assert.ok(page.button(page.roots.work, "Needs you · 1"));
  },

  async "the session's Chief question opens the inbox and never sends on one tap"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.run('openSession("s1")'); await settle();
    assert.ok(page.roots.detail.textContent.includes("Chief needs you"));
    assert.equal(page.button(page.roots.detail, "Use the suggestion"), undefined);
    page.button(page.roots.detail, "Do this, step by step").onclick(); await settle();
    assert.equal(page.roots.chief.hidden, false);
    assert.ok(page.roots.chief.textContent.includes("Step 1 of 2"));
    assert.equal(page.server.posts.length, 0);
  },

  async "a deferred question opened from its session shows that question, not another"() {
    const chief = inboxChief();
    chief.escalations[0].later_until_ms = Date.now() + 600000;
    chief.escalations.push({ ...chief.escalations[0], id: "d2", session_id: "s2", question: "OTHER QUESTION", later_until_ms: null });
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('openSession("s1")'); await settle();
    page.button(page.roots.detail, "Write my own answer").onclick(); await settle();
    assert.equal(page.run("inbox.decision"), "d1");
    assert.ok(page.roots.chief.textContent.includes("Your answer"));
  },

  async "the empty inbox shows a question that arrives while it is open"() {
    const chief = inboxChief(); chief.escalations = [];
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('openInbox()'); await settle();
    assert.ok(page.roots.chief.textContent.includes("Nothing needs you"));
    page.server.chief.escalations = inboxChief().escalations;
    await page.run("refresh()"); await settle();
    assert.ok(page.roots.chief.textContent.includes("! Chief needs you · 1 of 1"));
  },

  async "an uncertain Later does not block answering the question"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.server.chiefPost = async () => ({ ok: false, status: 503, json: async () => ({ ok: false, message: "Host lost", outcome_uncertain: true }) });
    page.run('openInbox()'); await settle();
    await page.button(page.roots.chief, "Later").onclick(); await settle();
    page.button(page.roots.chief, "Write my own").onclick(); await settle();
    const input = page.roots.chief.querySelector(".chief-input"); input.value = "Done"; input.oninput();
    assert.equal(page.button(page.roots.chief, "Send").disabled, false);
  },

  async "a typed phone answer survives its question changing"() {
    const page = await startPage({ sessions: SESSIONS, chief: inboxChief() });
    page.run('openInbox()'); await settle();
    page.button(page.roots.chief, "Write my own").onclick(); await settle();
    const input = page.roots.chief.querySelector(".chief-input"); input.value = "My long answer"; input.oninput();
    page.server.chief.escalations[0].revision = 4;
    await page.run("refresh()"); await settle();
    const text = page.roots.chief.textContent;
    assert.ok(text.includes("The question changed; your draft is kept below.") && text.includes("My long answer"), text);
    page.button(page.roots.chief, "Write my own").onclick(); await settle();
    assert.equal(page.roots.chief.querySelector(".chief-input").value, "My long answer");
    await page.button(page.roots.chief, "Send").onclick(); await page.button(page.roots.chief, "Tap again to send").onclick(); await settle();
    const post = page.server.posts.find(post => post.path === "/api/chief");
    assert.deepEqual([post.body.action.answer, post.body.action.expected_revision], ["My long answer", 4]);
  },

  async "Chief escalations are session scoped and submit the exact suggestion once"() {
    const chief = { status: "Active", counts: [], answers: [], habits: [], escalations: [
      { id: "d1", revision: 4, session_id: "s1", category: "security", question: "Rotate token?", recommendation: "Security needs you.", suggested_answer: "Rotate it." },
      { id: "d2", revision: 1, session_id: "s2", category: "scope", question: "OTHER SESSION", suggested_answer: null },
    ] };
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('openSession("s1")'); await settle();
    assert.ok(page.roots.detail.textContent.includes("Chief needs you · Security"));
    assert.ok(!page.roots.detail.textContent.includes("OTHER SESSION"));
    page.button(page.roots.detail, "Do this, step by step").onclick(); await settle();
    await page.button(page.roots.chief, "Send").onclick();
    const use = page.button(page.roots.chief, "Tap again to send");
    await use.onclick(); await use.onclick(); await settle();
    const posts = page.server.posts.filter(post => post.path === "/api/chief");
    assert.equal(posts.length, 1);
    assert.equal(posts[0].body.action.answer, "Rotate it.");
    assert.equal(posts[0].body.action.expected_revision, 4);
    assert.deepEqual(posts[0].body.expected, chief.escalations[0]);
  },

  async "a Chief answer draft survives refresh and Enter does not send"() {
    const chief = { status: "Active", counts: [], answers: [], habits: [], escalations: [
      { id: "d1", revision: 4, session_id: "s1", category: "scope", question: "Continue?", suggested_answer: null },
    ] };
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('openSession("s1")'); await settle();
    assert.equal(page.button(page.roots.detail, "Do this, step by step"), undefined);
    page.button(page.roots.detail, "Write my own answer").onclick(); await settle();
    const input = page.roots.chief.querySelector(".chief-input");
    input.value = "Keep this draft"; input.oninput();
    input.onkeydown?.({ key: "Enter", preventDefault() {} });
    await page.run("refresh()"); await settle();
    assert.equal(page.roots.chief.querySelector(".chief-input"), input);
    assert.equal(input.value, "Keep this draft");
    assert.equal(page.server.posts.length, 0);
    await page.button(page.roots.chief, "Send").onclick();
    const oldSend = page.button(page.roots.chief, "Tap again to send");
    page.server.chief.escalations[0].revision = 5;
    await page.run("refresh()"); await settle();
    await oldSend.onclick();
    assert.equal(page.server.posts.length, 0, "a stale button cannot answer a refreshed decision");
  },

  async "Chief answers navigate back to the same session and overrule explicitly"() {
    const chief = { status: "Active", counts: [{ session_id: "s1", count: 1 }], escalations: [], habits: [], answers: [
      { id: "a1", session_id: "s1", project: "path:/Shop", question: "Which fix?", answer: "Use B.", reason: "Smallest fix", source: "chief", delivery: "received", overrule: null, habits: [], created_at_ms: 1 },
    ] };
    const page = await startPage({ sessions: SESSIONS, chief });
    page.run('openSession("s1")'); await settle();
    page.button(page.roots.detail, "Chief ✓1 · answers").onclick(); await settle();
    assert.equal(page.roots.detail.inert, true, "only the Chief page is keyboard reachable");
    assert.ok(page.roots.chief.textContent.includes("Which fix?"));
    page.button(page.roots.chief, "Overrule").onclick();
    const input = page.roots.chief.querySelector(".chief-input"); input.value = "Use C."; input.oninput();
    assert.ok(page.roots.chief.textContent.includes("Becomes a habit (Shop): Use C."));
    await page.button(page.roots.chief, "Send correction").onclick();
    assert.equal(page.server.posts[0].body.action.correction, "Use C.");
    page.history.back(); await settle();
    assert.equal(page.run("opened"), "s1");
    assert.equal(page.roots.chief.hidden, true);
    assert.equal(page.roots.detail.hidden, false);
    assert.equal(page.roots.detail.inert, false);
  },

  async "Chief failure is visible without making the session list offline"() {
    const page = await startPage({ sessions: SESSIONS }); page.server.chiefError = true;
    await page.run("refresh()"); page.run('openSession("s1")'); await settle();
    assert.ok(page.roots.detail.textContent.includes("Chief unavailable"));
    assert.equal(page.roots.updated.dataset.state, "online");
    assert.equal(page.button(page.roots.detail, "Use the suggestion"), undefined);
  },

  async "Chief habits delete once and offer undo with the refreshed deleted record"() {
    const habit = { id: "h1", text: "Run tests first", project: "path:/Shop", state: "active", created_at_ms: Date.now(), changed_at_ms: 1 };
    const page = await startPage({ sessions: SESSIONS, chief: { status: "Active", counts: [], answers: [], escalations: [], habits: [habit] } });
    page.run('openChief("habits")'); await settle();
    const remove = page.button(page.roots.chief, "Delete");
    await remove.onclick(); await remove.onclick(); await settle();
    assert.equal(page.server.posts.length, 1);
    assert.equal(page.server.posts[0].body.action.action, "delete_habit");
    page.server.chief.habits[0].state = "deleted"; await page.run("refresh()"); await settle();
    await page.button(page.roots.chief, "Undo delete").onclick();
    assert.equal(page.server.posts[1].body.action.action, "restore_habit");
    assert.equal(page.server.posts[1].body.expected.state, "deleted");
  },

  async "an empty Chief log says so"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openChief("answers", "s1")'); await settle();
    assert.ok(page.roots.chief.textContent.includes("No Chief answers yet"));
  },
  async "the back gesture returns from a session to the list"() {
    const page = await startPage({ sessions: SESSIONS });
    page.roots.list.querySelector(".show-quiet").onclick();
    page.roots.list.querySelector(".session-card").onclick(); await settle();
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
    assert.equal(page.document.title, "(2) Agent Cockpit");
  },

  async "the tabs are Sessions, Todo, Ideas and Status, with the column's counts"() {
    const page = await startPage({ sessions: SESSIONS });
    page.server.counts = { todo: 5, ideas: 3 };
    await page.run("refresh()");
    const tabs = page.roots.tabs.querySelectorAll("button");
    assert.deepEqual(tabs.map(tab => tab.id), ["tab-sessions", "tab-todo", "tab-ideas", "tab-status"]);
    assert.equal(page.document.getElementById("tab-todo").querySelector(".badge").textContent, "5");
    assert.equal(page.document.getElementById("tab-ideas").querySelector(".badge").textContent, "3");
    assert.equal(page.document.getElementById("tab-status").querySelector(".badge"), null);
  },

  async "the badge disappears once nothing needs you"() {
    const page = await startPage({ sessions: [{ session: "s1", project: "Shop", state: "needs-you", features: [] }] });
    page.server.sessions = SESSIONS;
    await page.run("refresh()");
    const tab = page.document.getElementById("tab-sessions");
    assert.equal(tab.querySelector(".badge").hidden, true);
    assert.equal(tab.getAttribute("aria-label"), "Sessions");
    assert.equal(page.document.title, "Agent Cockpit");
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

  async "an image line in a reply shows the image by its place in the reply"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "a1", role: "assistant",
      text: "Before:\n![Login](design/login.png)\nsee ![inline](x.png)\n```\n![code](no.png)\n```\n![](\"onerror=x.png)" }] });
    page.run('openSession("s1")'); await settle();
    const images = page.document.getElementById("conversation").querySelectorAll("img");
    assert.deepEqual(images.map(image => image.src), ["/api/image/s1/a1/0", "/api/image/s1/a1/1"]);
    assert.equal(images[0].alt, "Login");
    assert.equal(images[1].alt, "\"onerror=x.png", "an image without alt text is named by its path, as text");
    assert.match(page.document.getElementById("conversation").textContent, /see !\[inline\]\(x\.png\)/);
  },

  async "an image that cannot load says which file"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "a1", role: "assistant", text: "![Login](design/login.png)" }] });
    page.run('openSession("s1")'); await settle();
    const figure = page.document.getElementById("conversation").querySelector("figure");
    figure.querySelector("img").onerror();
    assert.equal(figure.querySelector("img").hidden, true);
    assert.equal(figure.querySelector("figcaption").textContent, "Image not shown: design/login.png");
  },

  async "your own image lines stay literal"() {
    const page = await startPage({ sessions: SESSIONS, messages: [{ id: "u1", role: "user", text: "![mine](design/login.png)" }] });
    page.run('openSession("s1")'); await settle();
    assert.equal(page.document.getElementById("conversation").querySelectorAll("img").length, 0);
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

  async "a card leads with what the session does, then project › task"() {
    const page = await startPage({ sessions: [{ ...SESSIONS[0], state: "working", group: "working", doing: "Running the tests",
                                                title: "Remote access to desktop from phone" }] });
    const card = page.roots.list.querySelector(".session-card");
    assert.equal(card.querySelector(".doing").textContent, "Running the tests");
    assert.equal(card.querySelector(".card-project").textContent, "Shop");
    assert.equal(card.querySelector(".card-task").textContent, "Remote access to desktop from phone");
    assert.equal(card.querySelector(".ask-bar"), null, "only a card that needs you has an ask");
    page.run('openSession("s1")'); await settle();
    assert.equal(page.document.getElementById("chat-heading").textContent, "Remote access to desktop from phone");
    assert.equal(page.document.getElementById("chat-status").textContent, "Shop · Working");
  },

  async "a card without a title shows its project alone"() {
    const page = await startPage({ sessions: [{ ...SESSIONS[0], state: "working", group: "working" }] });
    const card = page.roots.list.querySelector(".session-card");
    assert.equal(card.querySelector(".card-project").textContent, "Shop");
    assert.equal(card.querySelector(".card-task"), null);
  },

  async "sessions are grouped by attention, oldest ask first, quiet folded"() {
    const now = Math.floor(Date.now() / 1000);
    const page = await startPage({
      sessions: [{ session: "q", project: "Notes", state: "waiting", group: "quiet", since: now, features: [] },
                 { session: "w", project: "Shop", state: "working", group: "working", since: now, features: [] },
                 { session: "n2", project: "Blog", state: "needs-you", group: "needs-you", request: "Which layout?", since: now - 60, features: [] },
                 { session: "n1", project: "Cockpit", state: "waiting", group: "quiet", since: now - 600, features: [] }],
      approvals: [{ id: "a1", session: "n1", project: "Cockpit", command: "cockpit apply" }],
    });
    const list = page.roots.list;
    assert.deepEqual(list.querySelectorAll(".group-heading").map(node => node.textContent), ["Needs you · 2", "Working · 1"]);
    assert.equal(list.querySelector(".need-pill").textContent, "2 need you");
    const asks = list.querySelectorAll(".ask-bar").map(bar => bar.textContent);
    assert.deepEqual(asks, ["Approve: cockpit applyReview ›", "Which layout?Answer ›"]);
    assert.equal(list.querySelectorAll(".session-card").length, 3, "quiet stays folded");
    assert.equal(list.querySelector(".quiet-row").children[0].textContent, "Quiet · 1 (finished)");
    list.querySelector(".show-quiet").onclick();
    assert.equal(page.roots.list.querySelectorAll(".session-card").length, 4);
  },

  async "a working card says how long since its agent updated its step"() {
    const page = await startPage({ sessions: [{ session: "w", project: "Shop", state: "working", group: "working", doing: "Bash  cargo test",
                                                stale_for: 18 * 60, features: [] }] });
    const card = page.roots.list.querySelector(".session-card");
    assert.equal(card.querySelector(".stale").textContent, "no update 18m");
  },

  async "a command's question shows what it printed and the question"() {
    const page = await startPage({ sessions: SESSIONS, inputs: [INPUT], inputOutput: "1 commit(s) for agent-cockpit:\n  fix: thing\n" }); await settle();
    const card = page.roots.approvals.querySelector("article");
    assert.equal(card.querySelector(".command").textContent, "cockpit apply /clone");
    assert.match(card.querySelector(".input-output").textContent, /fix: thing/);
    assert.equal(card.querySelector(".question").textContent, "Apply these 1 commit(s)? [y/N]");
  },

  async "yes to a command's question takes a second tap"() {
    const page = await startPage({ sessions: SESSIONS, inputs: [INPUT] }); await settle();
    const card = page.roots.approvals.querySelector("article");
    page.button(card, "Yes").onclick();
    assert.equal(page.server.posts.length, 0);
    await page.button(card, "Tap again for yes").onclick();
    assert.deepEqual(page.server.posts, [{ path: "/api/input/0123456789ab", body: { yes: true, prompt: "Apply these 1 commit(s)? [y/N]" } }]);
  },

  async "no to a command's question answers at once"() {
    const page = await startPage({ sessions: SESSIONS, inputs: [INPUT] }); await settle();
    await page.button(page.roots.approvals.querySelector("article"), "No").onclick();
    assert.deepEqual(page.server.posts, [{ path: "/api/input/0123456789ab", body: { yes: false, prompt: "Apply these 1 commit(s)? [y/N]" } }]);
  },

  async "a command's question counts in the badge"() {
    const page = await startPage({ sessions: SESSIONS, inputs: [INPUT] });
    assert.equal(page.document.getElementById("tab-sessions").querySelector(".badge").textContent, "1");
  },

  async "a reply keeps paragraphs and indentation as one message"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openSession("s1")'); await settle();
    const reply = page.document.getElementById("chat-reply");
    reply.value = "Fix the login bug\n- check the cookie\r\n\n  - add a test\t now\n"; reply.oninput();
    await page.button(page.roots.detail, "Send").onclick();
    assert.deepEqual(page.server.posts, [{ path: "/api/message/s1", body: { text: "Fix the login bug\n- check the cookie\n\n  - add a test  now" } }]);
  },

  async "the reply box no longer warns that paragraphs become spaces"() {
    const page = await startPage({ sessions: SESSIONS });
    page.run('openSession("s1")'); await settle();
    const reply = page.document.getElementById("chat-reply");
    reply.value = "two\nlines"; reply.oninput();
    assert.equal(page.roots.detail.querySelector("#reply-hint"), null);
    assert.doesNotMatch(page.roots.detail.textContent, /Line breaks are sent as spaces/);
  },

  async "a queued reply preserves blank lines"() {
    const page = await startPage({ sessions: [{ ...SESSIONS[0], state: "working" }] });
    page.run('openSession("s1")'); await settle();
    const reply = page.document.getElementById("chat-reply");
    reply.value = "First\n\n  second"; reply.oninput();
    await page.button(page.roots.detail, "Queue").onclick();
    assert.deepEqual(page.server.posts, [{ path: "/api/message/s1", body: { text: "First\n\n  second" } }]);
  },

  async "a multiline draft survives switching sessions"() {
    const page = await startPage({ sessions: [...SESSIONS, { ...SESSIONS[0], session: "s2" }] });
    page.run('openSession("s1")'); await settle();
    const reply = page.document.getElementById("chat-reply");
    reply.value = "First\n\n  second"; reply.oninput();
    page.run('openSession("s2")'); await settle();
    assert.equal(page.document.getElementById("chat-reply").value, "");
    page.run('openSession("s1")'); await settle();
    assert.equal(page.document.getElementById("chat-reply").value, "First\n\n  second");
  },

  async "a new session receives its first message with paragraphs intact"() {
    const page = await startPage({ sessions: SESSIONS });
    page.button(page.roots.starter, "+ New session").onclick(); await settle();
    const [project, agent] = page.roots.starter.querySelectorAll("select");
    project.value = "/shop"; agent.value = "claude";
    page.roots.starter.querySelector("textarea").value = "First\r\n\r\n  - second";
    await page.button(page.roots.starter, "Start").onclick();
    assert.deepEqual(page.server.posts, [{ path: "/api/sessions", body: { project: "/shop", agent: "claude", prompt: "First\n\n  - second", clone: true } }]);
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

  async "an asleep session shows its age and a Wake button in the list"() {
    const page = await startPage({ sessions: ASLEEP });
    page.roots.list.querySelector(".show-quiet").onclick();
    assert.match(page.roots.list.textContent, /☾Asleep2h00/);
    await page.button(page.roots.list, "Wake").onclick(); await settle();
    assert.deepEqual(page.server.requests.filter(request => request.path === "/api/wake/s1"), [{ path: "/api/wake/s1", method: "POST" }]);
  },

  async "an asleep session offers Wake and none of the buttons of a live one"() {
    const page = await startPage({ sessions: ASLEEP });
    page.run('openSession("s1")'); await settle();
    page.document.getElementById("session-more-toggle").onclick();
    assert.match(page.roots.detail.textContent, /asleep · 2h00/);
    await page.button(page.roots.detail, "Wake").onclick(); await settle();
    assert.equal(page.server.requests.some(request => request.path === "/api/wake/s1"), true);
    for (const label of ["Yes", "No", "Interrupt", "Remote Control: on", "Remote Control: off"]) {
      assert.equal(page.button(page.roots.detail, label), undefined, label);
    }
  },
};

const failures = [];
for (const [name, test] of Object.entries(tests)) {
  try { await test(); } catch (error) { error.message = `${name}: ${error.message}`; failures.push(error); }
}
if (failures.length) throw new AggregateError(failures, `${failures.length} mobile checks failed`);
console.log(`mobile page: ${Object.keys(tests).length} navigation, badge, formatting and reading checks passed`);
