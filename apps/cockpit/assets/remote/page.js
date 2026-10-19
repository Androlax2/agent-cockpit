// The phone page of claude-remote. Everything a session or Claude wrote is set as text
// (textContent), never as HTML: a task or a command cannot inject markup.
"use strict";

const GLYPHS = { "needs-you": "●", waiting: "◐", working: "◌", unknown: "?" };
let state = { sessions: [], approvals: [] };
let opened = null; // the session shown in detail
let armed = null;  // the approval whose Run, or the session whose Interrupt, waits for its second tap
let shownScreen = ""; // the screen as last fetched, escapes included

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

async function call(method, path, body) {
  const options = { method, headers: { "X-Claude-Remote": "1" } };
  if (body !== undefined) {
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  return response;
}

function age(since) {
  if (!since) return "";
  const minutes = Math.max(0, Math.round((Date.now() / 1000 - since) / 60));
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h${String(minutes % 60).padStart(2, "0")}`;
}

function renderApprovals() {
  const box = document.getElementById("approvals");
  box.replaceChildren();
  if (state.approvals.length === 0) return;
  box.append(element("h2", "", "Asks you to run"));
  for (const approval of state.approvals) {
    const card = element("div", "card approval");
    card.append(element("div", "project", `Claude · ${approval.project}`));
    card.append(element("div", "task", `in ${approval.cwd}`));
    card.append(element("pre", "command", approval.command));
    if (approval.why) card.append(element("div", "why", approval.why));
    if (approval.is_outside_its_folder) {
      card.append(element("div", "warning", `This runs outside the folder the session works in (${approval.started_in}).`));
    }
    const note = element("div", "note");
    const buttons = element("div", "buttons");
    const decline = element("button", "no", "Decline");
    const run = element("button", armed === approval.id ? "armed" : "run", armed === approval.id ? "Tap again to run" : "Run it");
    decline.onclick = () => decide(approval, false, note);
    run.onclick = () => {
      if (armed !== approval.id) { armed = approval.id; render(); return; }
      armed = null;
      decide(approval, true, note);
    };
    buttons.append(decline, run);
    card.append(buttons, note);
    box.append(card);
  }
}

function decide(approval, run, note) {
  return post(`/api/decide/${approval.id}`, { run, command: approval.command }, note);
}

function renderList() {
  const box = document.getElementById("list");
  box.replaceChildren(element("h2", "", "Sessions"));
  if (state.sessions.length === 0) box.append(element("div", "empty", "No session runs."));
  const order = { "needs-you": 0, waiting: 1, working: 2 };
  const sessions = [...state.sessions].sort((a, b) => (order[a.state] ?? 3) - (order[b.state] ?? 3));
  for (const session of sessions) {
    const card = element("div", `card ${session.state}`);
    const row = element("div", "row");
    row.append(element("span", "project", `${GLYPHS[session.state] || "·"} ${session.project}`));
    row.append(element("span", "state", `${session.state} ${age(session.since)}`));
    card.append(row);
    if (session.task) card.append(element("div", "task", session.task));
    if (session.request) card.append(element("div", "request", session.detail || session.request));
    if (session.queued) card.append(element("div", "task", `✉ ${session.queued} queued`));
    card.onclick = () => { opened = session.session; render(); loadScreen(); };
    box.append(card);
  }
}

// The terminal's 16 colours (Tokyo Night, as kitty's), then the 256-colour cube and greys.
const PALETTE = ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
                 "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"];

function colour256(index) {
  if (index < 16) return PALETTE[index];
  if (index >= 232) { const grey = 8 + (index - 232) * 10; return `rgb(${grey},${grey},${grey})`; }
  const cube = index - 16, step = (value) => (value === 0 ? 0 : 55 + value * 40);
  return `rgb(${step(Math.floor(cube / 36))},${step(Math.floor(cube / 6) % 6)},${step(cube % 6)})`;
}

// One SGR sequence's numbers applied to the running style. Only numbers become CSS values: nothing
// of the screen's text ever reaches a style or the markup.
function applySgr(codes, look) {
  for (let i = 0; i < codes.length; i++) {
    const code = codes[i];
    if (code === 0) Object.assign(look, { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false });
    else if (code === 1) look.bold = true;
    else if (code === 2) look.dim = true;
    else if (code === 3) look.italic = true;
    else if (code === 4) look.underline = true;
    else if (code === 7) look.inverse = true;
    else if (code === 22) { look.bold = false; look.dim = false; }
    else if (code === 23) look.italic = false;
    else if (code === 24) look.underline = false;
    else if (code === 27) look.inverse = false;
    else if (code >= 30 && code <= 37) look.fg = PALETTE[code - 30];
    else if (code >= 90 && code <= 97) look.fg = PALETTE[code - 90 + 8];
    else if (code === 39) look.fg = null;
    else if (code >= 40 && code <= 47) look.bg = PALETTE[code - 40];
    else if (code >= 100 && code <= 107) look.bg = PALETTE[code - 100 + 8];
    else if (code === 49) look.bg = null;
    else if ((code === 38 || code === 48) && codes[i + 1] === 5) {
      look[code === 38 ? "fg" : "bg"] = colour256((codes[i + 2] || 0) & 255);
      i += 2;
    } else if ((code === 38 || code === 48) && codes[i + 1] === 2) {
      const [r, g, b] = codes.slice(i + 2, i + 5).map((value) => (value || 0) & 255);
      look[code === 38 ? "fg" : "bg"] = `rgb(${r},${g},${b})`;
      i += 4;
    }
  }
}

function colouredNodes(text) {
  const nodes = [];
  const look = {};
  applySgr([0], look);
  for (const [index, part] of text.split(/\x1b\[([0-9;]*)m/).entries()) {
    if (index % 2 === 1) { applySgr(part === "" ? [0] : part.split(";").map(Number), look); continue; }
    if (part === "") continue;
    const span = element("span", "", part);
    const [fg, bg] = look.inverse ? [look.bg || "var(--bg)", look.fg || "var(--fg)"] : [look.fg, look.bg];
    if (fg) span.style.color = fg;
    if (bg) span.style.backgroundColor = bg;
    if (look.bold) span.style.fontWeight = "700";
    if (look.dim) span.style.opacity = "0.6";
    if (look.italic) span.style.fontStyle = "italic";
    if (look.underline) span.style.textDecoration = "underline";
    nodes.push(span);
  }
  return nodes;
}

// The session's screen, kept live: redrawn only when it changed, and following its newest lines
// unless Theo scrolled up to read.
async function loadScreen() {
  const screen = document.getElementById("screen");
  if (!screen || !opened) return;
  const response = await call("GET", `/api/screen/${opened}`);
  const text = (await response.text()).replace(/\s+$/, "");
  if (text === shownScreen) return;
  const isFollowing = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 24 || shownScreen === "";
  shownScreen = text;
  screen.replaceChildren(...colouredNodes(text));
  if (isFollowing) screen.scrollTop = screen.scrollHeight;
}

async function post(path, body, note) {
  note.textContent = "…";
  const response = await call("POST", path, body);
  const result = await response.json().catch(() => ({ message: "no answer" }));
  note.textContent = result.message;
  refresh();
  return result;
}

function renderDetail() {
  const box = document.getElementById("detail");
  const session = state.sessions.find((s) => s.session === opened);
  if (!session) { opened = null; box.hidden = true; return; }
  box.hidden = false;
  const back = element("button", "back", "‹ Sessions");
  back.onclick = () => { opened = null; render(); };
  const card = element("div", `card ${session.state}`);
  card.append(element("div", "project", `${session.project} · ${session.state}`));
  if (session.task) card.append(element("div", "task", session.task));
  const screen = element("pre", "", "…");
  screen.id = "screen";
  shownScreen = "";
  const note = element("div", "note");
  const controls = element("div", "buttons");
  if (session.state === "working" && isClaude) {
    const isArmed = armed === session.session;
    const interrupt = element("button", isArmed ? "armed" : "no", isArmed ? "Tap again to interrupt" : "Interrupt (Esc)");
    interrupt.onclick = () => {
      if (armed !== session.session) { armed = session.session; renderDetail(); loadScreen(); return; }
      armed = null;
      post(`/api/interrupt/${session.session}`, {}, note);
    };
    controls.append(interrupt);
  }
  if (isClaude) {
    const remoteControl = element("button", "no", session.remote_control ? "Remote Control: on" : "Remote Control: off");
    remoteControl.onclick = () => post(`/api/remote-control/${session.session}`, {}, note);
    controls.append(remoteControl);
  }
  card.append(controls);
  const isClaude = session.agent !== "opencode";
  if (!isClaude) card.append(element("div", "task", "opencode: answer and interrupt it in its pane on the computer."));
  if (session.state === "needs-you" && isClaude) {
    card.append(element("div", "request", session.detail || session.request));
    const buttons = element("div", "buttons");
    const no = element("button", "no", "No");
    const yes = element("button", "run", "Yes");
    no.onclick = () => post(`/api/answer/${session.session}`, { yes: false }, note);
    yes.onclick = () => post(`/api/answer/${session.session}`, { yes: true }, note);
    buttons.append(no, yes);
    card.append(buttons);
  }
  const typed = box.querySelector("textarea")?.value || "";
  const message = element("textarea");
  message.value = typed;
  message.placeholder = session.state === "working" ? "A message for the end of its turn" : "A message for Claude";
  const send = element("button", "run", session.state === "working" ? "Queue it" : "Send");
  send.onclick = async () => {
    const result = await post(`/api/message/${session.session}`, { text: message.value }, note);
    if (result.ok) message.value = "";
  };
  const sending = element("div", "buttons");
  sending.append(send);
  box.replaceChildren(back, card, screen, message, sending, note);
}

// ---- This phone: notifications, and pairing the Home Screen app ----

const isHomeScreenApp = window.navigator.standalone === true || window.matchMedia("(display-mode: standalone)").matches;
const canPush = "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
let phoneLook = null; // what the phone section shows, to redraw it only when that changes

function keyBytes(key) {
  const text = atob(key.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (key.length % 4)) % 4));
  return Uint8Array.from(text, (character) => character.charCodeAt(0));
}

async function turnNotificationsOn(note) {
  note.textContent = "…";
  if (await Notification.requestPermission() !== "granted") {
    note.textContent = "Notifications are refused for this app: allow them in the phone's settings.";
    return;
  }
  const registration = await navigator.serviceWorker.register("/sw.js");
  await navigator.serviceWorker.ready;
  const { key } = await (await call("GET", "/api/push/key")).json();
  const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(key) });
  await post("/api/push/subscribe", subscription.toJSON(), note);
}

async function turnNotificationsOff(note) {
  const registration = await navigator.serviceWorker.getRegistration();
  const subscription = registration && await registration.pushManager.getSubscription();
  if (subscription) await subscription.unsubscribe();
  await post("/api/push/unsubscribe", {}, note);
}

function renderPhone() {
  const box = document.getElementById("phone");
  const look = JSON.stringify([opened === null, state.push]);
  if (look === phoneLook) return;
  phoneLook = look;
  box.hidden = opened !== null;
  const note = element("div", "note");
  const card = element("div", "card");
  const buttons = element("div", "buttons");
  if (canPush && (isHomeScreenApp || !/iPhone|iPad/.test(navigator.userAgent))) {
    card.append(element("div", "task", state.push ? "Notifications are on: a session that needs you or a command that asks to run." : "Notifications are off."));
    if (state.push) {
      const test = element("button", "no", "Send a test");
      const off = element("button", "no", "Turn off");
      test.onclick = () => post("/api/push/test", {}, note);
      off.onclick = () => turnNotificationsOff(note).catch((error) => { note.textContent = String(error); });
      buttons.append(test, off);
    } else {
      const on = element("button", "run", "Turn on notifications");
      on.onclick = () => turnNotificationsOn(note).catch((error) => { note.textContent = String(error); });
      buttons.append(on);
    }
  } else {
    // iOS gives notifications only to a page added to the Home Screen, and that app has cookies
    // of its own: it pairs with a code this paired page asks for.
    card.append(element("div", "task", "For notifications: Share → Add to Home Screen, open it from there, and paste the code below in it."));
    const code = element("div", "code");
    const get = element("button", "run", "Get a code for the Home Screen app");
    get.onclick = async () => {
      const result = await post("/api/pairing-code", {}, note);
      if (!result.ok) return;
      code.textContent = result.message;
      note.textContent = "One use, within 10 minutes.";
      await navigator.clipboard?.writeText(result.message).then(() => { note.textContent = "Copied. One use, within 10 minutes."; }).catch(() => {});
    };
    buttons.append(get);
    card.append(code);
  }
  card.append(buttons, note);
  box.replaceChildren(element("h2", "", "This phone"), card);
}

// An unpaired phone (the Home Screen app, first time): where to paste the code.
function renderPairing(message) {
  for (const id of ["approvals", "list", "detail", "phone"]) document.getElementById(id).hidden = true;
  const box = document.getElementById("pairing");
  if (!box.hidden) return;
  box.hidden = false;
  const input = element("input");
  input.placeholder = "Pairing code or link";
  input.autocapitalize = "off";
  const note = element("div", "note", message);
  const pair = element("button", "run", "Pair");
  pair.onclick = () => {
    const code = (input.value.trim().split("/pair/").pop() || "").trim();
    if (!/^[0-9a-f]{32}$/.test(code)) { note.textContent = "A code is 32 characters, 0-9 and a-f."; return; }
    window.location.href = `/pair/${code}`;
  };
  const buttons = element("div", "buttons");
  buttons.append(pair);
  box.replaceChildren(element("h2", "", "Pair this phone"), input, buttons, note);
}

function render() {
  renderApprovals();
  renderPhone();
  const detail = opened !== null;
  document.getElementById("list").hidden = detail;
  if (detail) renderDetail(); else document.getElementById("detail").hidden = true;
  if (!detail) renderList();
}

// What the detail view's card and buttons depend on, to tell when they need redrawing.
function detailLook() {
  const session = state.sessions.find((s) => s.session === opened);
  return session ? JSON.stringify([session.state, session.request, session.detail, session.remote_control, session.task]) : "";
}

async function refresh() {
  try {
    const response = await call("GET", "/api/state");
    if (response.status === 401) { renderPairing(await response.text()); return; }
    const before = detailLook();
    state = await response.json();
    document.getElementById("updated").textContent = new Date().toLocaleTimeString();
    // The detail view redraws only when what it shows changed, keeping what is typed.
    renderApprovals();
    renderPhone();
    if (opened === null) renderList();
    else if (detailLook() !== before) { renderDetail(); loadScreen(); }
  } catch (error) {
    document.getElementById("updated").textContent = "offline";
  }
}

// A tap on a notification opens its session: in a new window through the address, in an open one
// through the service worker's message.
function openSession(session) {
  if (!session) return;
  opened = session;
  render();
  loadScreen();
}
if (location.hash.startsWith("#session=")) opened = location.hash.slice("#session=".length);
navigator.serviceWorker?.addEventListener("message", (event) => openSession(event.data && event.data.session));
if (canPush) navigator.serviceWorker.register("/sw.js").catch(() => {});

refresh().then(render);
setInterval(refresh, 3000);
setInterval(() => { if (opened) loadScreen(); }, 1500);
