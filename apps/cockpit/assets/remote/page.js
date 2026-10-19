// The phone page of claude-remote. Everything a session or Claude wrote is set as text
// (textContent), never as HTML: a task or a command cannot inject markup.
"use strict";

const GLYPHS = { "needs-you": "●", waiting: "◐", working: "◌", unknown: "?" };
let state = { sessions: [], approvals: [] };
let opened = null; // the session shown in detail
let armed = null;  // the approval whose Run waits for its second tap

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

async function decide(approval, run, note) {
  note.textContent = run ? "running…" : "declining…";
  const response = await call("POST", `/api/decide/${approval.id}`, { run, command: approval.command });
  const result = await response.json().catch(() => ({ message: "no answer" }));
  note.textContent = result.message;
  refresh();
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

// The session's screen, kept live: redrawn only when it changed, and following its newest lines
// unless Theo scrolled up to read.
async function loadScreen() {
  const screen = document.getElementById("screen");
  if (!screen || !opened) return;
  const response = await call("GET", `/api/screen/${opened}`);
  const text = (await response.text()).replace(/\s+$/, "");
  if (text === screen.textContent) return;
  const isFollowing = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 24 || screen.textContent === "…";
  screen.textContent = text;
  if (isFollowing) screen.scrollTop = screen.scrollHeight;
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
  const note = element("div", "note");
  if (session.state === "needs-you") {
    card.append(element("div", "request", session.detail || session.request));
    const buttons = element("div", "buttons");
    const no = element("button", "no", "No");
    const yes = element("button", "run", "Yes");
    no.onclick = () => answer(session, false, note);
    yes.onclick = () => answer(session, true, note);
    buttons.append(no, yes);
    card.append(buttons);
  }
  const message = element("textarea");
  message.placeholder = session.state === "working" ? "A message for the end of its turn" : "A message for Claude";
  const send = element("button", "run", session.state === "working" ? "Queue it" : "Send");
  send.onclick = async () => {
    note.textContent = "…";
    const response = await call("POST", `/api/message/${session.session}`, { text: message.value });
    const result = await response.json().catch(() => ({ message: "no answer" }));
    note.textContent = result.message;
    if (result.ok) message.value = "";
    refresh();
  };
  const sending = element("div", "buttons");
  sending.append(send);
  box.replaceChildren(back, card, screen, message, sending, note);
}

async function answer(session, yes, note) {
  note.textContent = "…";
  const response = await call("POST", `/api/answer/${session.session}`, { yes });
  const result = await response.json().catch(() => ({ message: "no answer" }));
  note.textContent = result.message;
  refresh();
}

function render() {
  renderApprovals();
  const detail = opened !== null;
  document.getElementById("list").hidden = detail;
  if (detail) renderDetail(); else document.getElementById("detail").hidden = true;
  if (!detail) renderList();
}

async function refresh() {
  try {
    const response = await call("GET", "/api/state");
    if (response.status === 401) { document.body.textContent = await response.text(); return; }
    state = await response.json();
    document.getElementById("updated").textContent = new Date().toLocaleTimeString();
    // The detail view keeps what is typed: only the list and the approvals redraw on a poll.
    renderApprovals();
    if (opened === null) renderList();
  } catch (error) {
    document.getElementById("updated").textContent = "offline";
  }
}

refresh().then(render);
setInterval(refresh, 3000);
setInterval(() => { if (opened) loadScreen(); }, 1500);
