// The phone page of claude-remote. Everything a session or Claude wrote is set as text
// (textContent), never as HTML: a task or a command cannot inject markup.
"use strict";

const GLYPHS = { "needs-you": "●", waiting: "◐", working: "◌", unknown: "?" };
const STATE_LABELS = { "needs-you": "Needs you", waiting: "Waiting", working: "Working", unknown: "Unknown" };
let state = { sessions: [], approvals: [] };
let isPaired = false;
let opened = null; // the session shown in detail
let armed = null;  // the approval whose Run, or the session whose Interrupt, waits for its second tap
let shownScreen = ""; // the screen as last fetched, escapes included
let approvalsLook = null;
const pendingPosts = new Set();
let controlSequence = 0;
const drafts = new Map();
const sendingMessages = new Set();
let shownConversation = "";
let conversationRequest = 0;
const loadingConversations = new WeakMap();

function statusLabel(session) {
  if (session.needs_input) return "Your reply needed";
  return { "needs-you": "Approval needed", working: "Working", waiting: "Ready", unknown: "Connecting" }[session.state] || "Connecting";
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  if (tag === "button") node.type = "button";
  if (className === "note") node.setAttribute("role", "status");
  if (className === "warning") node.setAttribute("role", "alert");
  return node;
}

function labelControl(label, control) {
  const field = element("div", "field");
  const caption = element("label", "field-label", label);
  control.id = `field-${++controlSequence}`;
  caption.htmlFor = control.id;
  field.append(caption, control);
  return field;
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
  const look = JSON.stringify([state.approvals, armed, opened, tab, isPaired]);
  if (look === approvalsLook) return;
  approvalsLook = look;
  const previousCards = Array.from(box.querySelectorAll(".approval"));
  box.replaceChildren();
  box.hidden = opened !== null && tab === "sessions";
  if (box.hidden) return;
  if (state.approvals.length === 0) return;
  box.append(element("h2", "", "Commands needing you"));
  for (const approval of state.approvals) {
    const previous = previousCards.find(card => card.dataset.id === approval.id && card.dataset.command === approval.command);
    box.append(previous || approvalCard(approval));
  }
}

function approvalCard(approval) {
  const card = element("article", "card approval");
  card.dataset.id = approval.id;
  card.dataset.command = approval.command;
  card.append(element("div", "project", `${approval.project} · Run this command?`));
  if (approval.why) card.append(element("div", "why", approval.why));
  card.append(element("pre", "command", approval.command));
  card.append(element("div", "task", `In ${approval.cwd || "its project folder"}`));
  if (approval.is_outside_its_folder) card.append(element("div", "warning", `Outside the session folder: ${approval.started_in}`));
  const note = element("div", "note");
  note.setAttribute("role", "status");
  const buttons = element("div", "buttons");
  const decline = element("button", "no", "Decline");
  const confirmation = `${approval.id}:${approval.command}`;
  const run = element("button", armed === confirmation ? "armed" : "run", armed === confirmation ? "Tap again to run" : "Run");
  let isSending = false;
  const submit = async (isRun) => {
    if (isSending) return;
    isSending = true; run.disabled = true; decline.disabled = true;
    const result = await decide(approval, isRun, note);
    if (!result.ok) { isSending = false; run.disabled = false; decline.disabled = false; }
    else note.textContent = isRun ? "Approved. Waiting for command output." : "Declined.";
  };
  decline.onclick = () => submit(false);
  run.onclick = () => {
    if (armed !== confirmation) {
      armed = confirmation; run.textContent = "Tap again to run"; run.className = "armed"; return;
    }
    armed = null;
    return submit(true);
  };
  buttons.append(decline, run); card.append(buttons, note);
  return card;
}

function decide(approval, run, note) {
  return post(`/api/decide/${approval.id}`, { run, command: approval.command }, note);
}

function renderList() {
  const box = document.getElementById("list");
  box.replaceChildren(element("h2", "", "Sessions"));
  if (state.sessions.length === 0) box.append(element("div", "empty", "No active sessions. Start a new session above to get going."));
  else {
    const needsYou = state.sessions.filter((session) => session.state === "needs-you").length;
    box.append(element("p", "overview", `${state.sessions.length} active · ${needsYou ? `${needsYou} need you` : "Nothing needs your attention"}`));
  }
  const order = { "needs-you": 0, waiting: 1, working: 2 };
  const sessions = [...state.sessions].sort((a, b) => (order[a.state] ?? 3) - (order[b.state] ?? 3));
  for (const session of sessions) {
    const card = element("button", `card session-card ${session.state}`);
    const row = element("span", "row");
    row.append(element("span", "project", session.project));
    row.append(element("span", "state", statusLabel(session)));
    card.append(row);
    if (session.task) card.append(element("span", "task", session.task));
    if (session.needs_input || session.request) card.append(element("span", "preview", session.detail || session.request || "Open to reply"));
    if (session.queued) card.append(element("span", "task", `${session.queued} message${session.queued === 1 ? "" : "s"} queued`));
    if (session.state === "waiting" && session.outcome) card.append(element("span", "outcome", session.outcome));
    card.onclick = () => openSession(session.session);
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
  const terminal = document.getElementById("terminal-details");
  const session = opened;
  if (!screen || !session || !terminal?.open) return;
  const note = document.getElementById("screen-note");
  try {
    const response = await call("GET", `/api/screen/${session}`);
    const text = (await response.text()).replace(/\s+$/, "");
    if (session !== opened || screen !== document.getElementById("screen")) return;
    if (!response.ok) throw new Error(text);
    note.textContent = "";
    if (text === shownScreen) return;
    const isFollowing = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 24 || shownScreen === "";
    shownScreen = text;
    screen.replaceChildren(...colouredNodes(text));
    if (isFollowing) screen.scrollTop = screen.scrollHeight;
  } catch (error) {
    if (screen === document.getElementById("screen")) note.textContent = `Failed to update the terminal: ${error.message || error}`;
  }
}

async function loadConversation() {
  const session = opened;
  const conversation = document.getElementById("conversation");
  if (!isPaired || !session || !conversation || tab !== "sessions") return;
  if (loadingConversations.has(conversation)) return loadingConversations.get(conversation);
  const loading = (async () => {
    const request = ++conversationRequest;
    try {
      const response = await call("GET", `/api/conversation/${session}`);
      const history = await response.json();
      if (request !== conversationRequest || session !== opened || conversation !== document.getElementById("conversation")) return;
      if (!response.ok) throw new Error(history.message || "Conversation unavailable. Open Terminal details below.");
      const signature = JSON.stringify(history);
      document.getElementById("conversation-error").textContent = "";
      if (signature === shownConversation) return;
      const isFollowing = shownConversation === "" || conversation.scrollTop + conversation.clientHeight >= conversation.scrollHeight - 32;
      const anchor = isFollowing ? null : Array.from(conversation.children).find(turn => turn.dataset.messageId
        && turn.getBoundingClientRect().bottom > conversation.getBoundingClientRect().top);
      const anchorTop = anchor?.getBoundingClientRect().top;
      shownConversation = signature;
      const turns = history.messages.map(message => {
        const turn = element("article", `chat-turn ${message.role === "user" ? "from-you" : "from-agent"}`);
        turn.dataset.messageId = message.id;
        turn.append(element("div", "speaker", message.role === "user" ? "You" : "Agent"), element("div", "message-text", message.text));
        return turn;
      });
      if (history.truncated) turns.unshift(element("p", "note", "Showing recent messages."));
      if (turns.length === 0) turns.push(element("p", "empty", "No messages yet. Start the conversation below."));
      conversation.replaceChildren(...turns);
      if (isFollowing) conversation.scrollTop = conversation.scrollHeight;
      else if (anchor) {
        const restored = turns.find(turn => turn.dataset.messageId === anchor.dataset.messageId);
        if (restored) conversation.scrollTop += restored.getBoundingClientRect().top - anchorTop;
        else {
          conversation.scrollTop = 0;
          document.getElementById("conversation-error").textContent = "The message you were reading is outside the recent history window.";
        }
      }
    } catch (error) {
      if (request !== conversationRequest || session !== opened || conversation !== document.getElementById("conversation")) return;
      const warning = document.getElementById("conversation-error");
      warning.textContent = `${error.message} Try again, or open Terminal details.`;
    } finally {
      loadingConversations.delete(conversation);
    }
  })();
  loadingConversations.set(conversation, loading);
  return loading;
}

async function post(path, body, note) {
  if (pendingPosts.has(path)) return { ok: false, message: "This action is still pending." };
  pendingPosts.add(path);
  note.textContent = "…";
  note.dataset.kind = "pending";
  try {
    const response = await call("POST", path, body);
    if (response.status === 401) {
      const message = await response.text();
      isPaired = false;
      renderPairing(message);
      return { ok: false, message };
    }
    const result = await response.json();
    if (!response.ok) result.ok = false;
    note.textContent = result.message;
    note.dataset.kind = result.ok ? "success" : "error";
    refresh();
    return result;
  } catch (error) {
    const message = `Failed to complete the action: ${error.message || error}. Check the connection before trying again.`;
    note.textContent = message;
    note.dataset.kind = "error";
    return { ok: false, message };
  } finally {
    pendingPosts.delete(path);
  }
}

function renderDetail() {
  if (!isPaired) return;
  const box = document.getElementById("detail");
  const session = state.sessions.find((s) => s.session === opened);
  if (!session) { opened = null; box.hidden = true; return; }
  box.hidden = false;
  const isNewSession = box.dataset.session !== session.session;
  if (isNewSession) {
    box.dataset.session = session.session;
    shownScreen = ""; shownConversation = "";
    const header = element("div", "chat-header");
    const back = element("button", "back", "‹ Sessions");
    back.onclick = () => { opened = null; box.dataset.session = ""; render(); };
    const heading = element("h2", "chat-heading"); heading.id = "chat-heading";
    const status = element("div", "chat-status"); status.id = "chat-status"; status.setAttribute("role", "status");
    header.append(back, heading, status);
    const conversation = element("section", "conversation"); conversation.id = "conversation";
    conversation.setAttribute("aria-label", "Conversation"); conversation.setAttribute("tabindex", "0");
    conversation.append(element("p", "empty", "Loading conversation…"));
    const error = element("div", "note"); error.id = "conversation-error"; error.setAttribute("role", "status");
    const actions = element("section", "chat-actions"); actions.id = "chat-actions"; actions.setAttribute("aria-label", "Needs your attention");
    const composer = element("form", "composer");
    const label = element("label", "reply-label", "Your message"); label.setAttribute("for", "chat-reply");
    const message = element("textarea"); message.id = "chat-reply";
    message.value = drafts.get(session.session) || "";
    message.maxLength = 4000; message.rows = 2;
    message.setAttribute("aria-describedby", "send-note");
    message.oninput = () => drafts.set(session.session, message.value);
    const send = element("button", "run", "Send"); send.id = "chat-send"; send.type = "submit";
    const note = element("div", "note"); note.id = "send-note"; note.setAttribute("role", "status");
    const submit = async (event) => {
      event?.preventDefault();
      if (sendingMessages.has(session.session)) return;
      const submitted = message.value;
      if (!submitted.trim()) { note.textContent = "Write a message first."; message.focus(); return; }
      sendingMessages.add(session.session); send.disabled = true; send.textContent = "Sending…";
      const result = await post(`/api/message/${session.session}`, { text: submitted }, note);
      sendingMessages.delete(session.session);
      if (result.ok && drafts.get(session.session) === submitted) {
        drafts.delete(session.session); message.value = "";
        if (opened === session.session) document.getElementById("chat-reply").value = "";
      }
      if (opened === session.session) { renderDetail(); loadConversation(); }
    };
    composer.onsubmit = submit;
    send.onclick = submit;
    composer.append(label, message, send, note);
    const options = element("details", "session-options");
    options.append(element("summary", "", "Session options"));
    const controls = element("div"); controls.id = "session-controls"; options.append(controls);
    const terminal = element("details", "terminal-details"); terminal.id = "terminal-details";
    terminal.append(element("summary", "", "Terminal details"));
    const screen = element("pre", "", "Open to load terminal output."); screen.id = "screen";
    screen.setAttribute("aria-label", "Session terminal output"); screen.tabIndex = 0;
    const screenNote = element("div", "note"); screenNote.id = "screen-note";
    const refreshTerminal = element("button", "", "Refresh terminal"); refreshTerminal.onclick = loadScreen;
    terminal.append(refreshTerminal, screen, screenNote); terminal.ontoggle = () => { if (terminal.open) loadScreen(); };
    box.replaceChildren(header, conversation, error, actions, composer, options, terminal);
    loadConversation();
  }
  document.getElementById("chat-heading").textContent = session.project;
  document.getElementById("chat-status").textContent = `${statusLabel(session)}${session.queued ? ` · ${session.queued} queued` : ""}`;
  const send = document.getElementById("chat-send");
  const isDialog = session.state === "needs-you" && !session.needs_input;
  send.textContent = sendingMessages.has(session.session) ? "Sending…" : session.state === "working" ? "Queue" : "Send";
  send.disabled = sendingMessages.has(session.session) || isDialog;
  document.getElementById("chat-reply").placeholder = session.state === "working" ? "Sent after this turn finishes" : "Reply to the agent…";
  const actions = document.getElementById("chat-actions");
  const approvals = state.approvals.filter(approval => approval.session === session.session);
  const actionLook = JSON.stringify([approvals, isDialog, session.detail, session.request, session.features]);
  if (actions.dataset.look !== actionLook) {
    actions.dataset.look = actionLook;
    actions.replaceChildren(...approvals.map(approvalCard));
    if (isDialog) {
      const prompt = element("div", "card approval");
      prompt.append(element("div", "project", "Permission needed"), element("div", "request", session.detail || session.request || "Answer the dialog before sending a message."));
      const note = element("div", "note"); note.setAttribute("role", "status");
      if ((session.features || []).includes("dialogs")) {
        const buttons = element("div", "buttons");
        for (const [label, yes] of [["No", false], ["Yes", true]]) {
          const button = element("button", yes ? "run" : "no", label);
          button.onclick = () => post(`/api/answer/${session.session}`, { yes }, note);
          buttons.append(button);
        }
        prompt.append(buttons);
      } else prompt.append(element("p", "task", "This agent’s permission dialog must be answered on the computer."));
      prompt.append(note); actions.append(prompt);
    }
  }
  const controls = document.getElementById("session-controls");
  const controlLook = JSON.stringify([session.state, session.remote_control, session.features, state.push, state.watched, armed]);
  if (controls.dataset.look === controlLook) return;
  controls.dataset.look = controlLook;
  controls.replaceChildren();
  const note = element("div", "note"); note.setAttribute("role", "status");
  // What its agent can do (cockpit::agents): the server refuses the rest anyway.
  const can = (feature) => (session.features || []).includes(feature);
  if (session.state === "working" && can("interrupt")) {
    const isArmed = armed === session.session;
    const interrupt = element("button", isArmed ? "armed" : "no", isArmed ? "Tap again to interrupt" : "Interrupt");
    interrupt.onclick = () => {
      if (armed !== session.session) { armed = session.session; renderDetail(); loadScreen(); return; }
      armed = null;
      post(`/api/interrupt/${session.session}`, {}, note);
    };
    controls.append(interrupt);
  }
  if (can("remote_control")) {
    const remoteControl = element("button", "no", session.remote_control ? "Remote Control: on" : "Remote Control: off");
    remoteControl.onclick = () => post(`/api/remote-control/${session.session}`, {}, note);
    controls.append(remoteControl);
  }
  if (state.push) {
    const isWatched = (state.watched || []).includes(session.session);
    const watch = element("button", "no", isWatched ? "Tell me when it finishes: on" : "Tell me when it finishes: off");
    watch.onclick = () => post(`/api/watch/${session.session}`, { on: !isWatched }, note);
    const watching = element("div", "buttons");
    watching.append(watch);
    controls.append(watching);
  }
  controls.append(note);
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
  const box = document.getElementById("phone-content");
  const look = JSON.stringify([opened === null, state.push, state.away]);
  if (look === phoneLook) return;
  phoneLook = look;
  document.getElementById("phone").hidden = opened !== null;
  const note = element("div", "note");
  const card = element("div", "card");
  const buttons = element("div", "buttons");
  if (canPush && (isHomeScreenApp || !/iPhone|iPad/.test(navigator.userAgent))) {
    card.append(element("div", "task", state.push
      ? `Notifications are on while the computer is locked: a session that needs you, a command that asks to run, a turn that ends. At the desk, only the sessions you ring.${state.away ? " (Locked now.)" : ""}`
      : "Notifications are off."));
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
  for (const id of ["approvals", "list", "detail", "phone", "starter", "work", "tabs"]) document.getElementById(id).hidden = true;
  const box = document.getElementById("pairing");
  if (!box.hidden) return;
  box.hidden = false;
  const input = element("input");
  input.placeholder = "123 456";
  input.inputMode = "numeric";
  input.autocomplete = "one-time-code";
  input.autocapitalize = "off";
  const hint = element("div", "task", "On the computer: S, Pair a phone. Type the 6 digits it shows (or paste a code from Safari).");
  const note = element("div", "note", message);
  const pair = element("button", "run", "Pair");
  pair.onclick = async () => {
    const typed = input.value.replace(/\s/g, "");
    if (/^[0-9]{6}$/.test(typed)) {
      const result = await post("/pair-code", { code: typed }, note);
      if (result.ok) { window.location.reload(); return; }
      note.textContent = result.message;
      return;
    }
    const code = typed.split("/pair/").pop() || "";
    if (!/^[0-9a-f]{32}$/.test(code)) { note.textContent = "Type the 6 digits the computer shows."; return; }
    window.location.href = `/pair/${code}`;
  };
  const buttons = element("div", "buttons");
  buttons.append(pair);
  box.replaceChildren(element("h2", "", "Pair this phone"), hint, labelControl("Pairing code", input), buttons, note);
}

// ---- Tabs: Sessions, Todo, Pull requests, Usage ----

const TABS = [["sessions", "Sessions"], ["todo", "Todo"], ["pulls", "Pull requests"], ["usage", "Usage"]];
let tab = "sessions";
let projectChoices = null; // fetched once: the projects a session or an item can go to

async function getJson(path) {
  const response = await call("GET", path);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

async function projects() {
  if (projectChoices === null) projectChoices = await getJson("/api/projects");
  return projectChoices;
}

// The agents a new session can run (cockpit::agents, from /api/agents), which the server checks.
function agentPicker(choices) {
  const picker = element("select");
  for (const agent of choices) {
    const option = element("option", "", agent.label);
    option.value = agent.name;
    picker.append(option);
  }
  return picker;
}

function projectPicker(choices) {
  const picker = element("select");
  for (const project of choices) {
    const option = element("option", "", project.name);
    option.value = project.path;
    picker.append(option);
  }
  return picker;
}

function renderTabs() {
  const bar = document.getElementById("tabs");
  document.getElementById("view-title").textContent = TABS.find(([name]) => name === tab)[1];
  bar.replaceChildren(...TABS.map(([name, label]) => {
    const button = element("button", name === tab ? "tab active" : "tab", label);
    if (name === tab) button.setAttribute("aria-current", "page");
    button.onclick = () => showTab(name);
    return button;
  }));
}

function showTab(name) {
  tab = name;
  opened = null;
  renderTabs();
  const isSessions = name === "sessions";
  for (const id of ["list", "phone", "starter"]) document.getElementById(id).hidden = !isSessions;
  document.getElementById("detail").hidden = true;
  document.getElementById("work").hidden = isSessions;
  if (isSessions) { phoneLook = null; render(); return; }
  const box = element("div");
  document.getElementById("work").replaceChildren(box);
  box.setAttribute("aria-busy", "true");
  box.replaceChildren(element("div", "loading", `Loading ${TABS.find(([key]) => key === name)[1].toLowerCase()}…`));
  const views = { todo: renderTodos, pulls: renderPulls, usage: renderUsage };
  views[name](box).catch((error) => {
    const retry = element("button", "no", "Retry");
    retry.onclick = () => showTab(name);
    box.replaceChildren(element("div", "warning", String(error.message || error)), retry);
  }).finally(() => box.removeAttribute("aria-busy"));
}

// A new session, out of sight on the computer, with its first message.
async function renderStarter() {
  const box = document.getElementById("starter");
  if (box.dataset.open !== "yes") {
    const open = element("button", "no", "+ New session");
    open.onclick = () => {
      box.dataset.open = "yes";
      renderStarter().catch((error) => box.replaceChildren(element("div", "warning", String(error))));
    };
    box.replaceChildren(open);
    return;
  }
  const [projectList, agentChoices] = await Promise.all([projects(), getJson("/api/agents")]);
  const picker = projectPicker(projectList);
  const agent = agentPicker(agentChoices);
  const prompt = element("textarea");
  prompt.placeholder = "What should the session do?";
  const inClone = element("input");
  inClone.type = "checkbox";
  inClone.checked = true;
  const label = element("label", "check");
  label.append(inClone, element("span", "", " in its own clone (a branch of its own)"));
  const note = element("div", "note");
  const start = element("button", "run", "Start");
  start.onclick = async () => {
    const result = await post("/api/sessions", { project: picker.value, agent: agent.value, prompt: prompt.value, clone: inClone.checked }, note);
    if (result.ok) prompt.value = "";
  };
  const close = element("button", "no", "Close");
  close.onclick = () => { box.dataset.open = ""; renderStarter(); };
  const buttons = element("div", "buttons");
  buttons.append(close, start);
  box.replaceChildren(element("h2", "", "New session"), labelControl("Project", picker), labelControl("Agent", agent),
                      labelControl("First message", prompt), label, buttons, note);
}

async function renderTodos(box) {
  const [todos, choices, agentChoices] = await Promise.all([getJson("/api/todos"), projects(), getJson("/api/agents")]);
  const picker = projectPicker(choices);
  const text = element("input");
  text.placeholder = "A new item";
  const notes = element("textarea");
  notes.placeholder = "Notes (optional)";
  const note = element("div", "note");
  const add = element("button", "run", "Add");
  add.onclick = async () => {
    const result = await post("/api/todos", { project: picker.value, text: text.value, notes: notes.value }, note);
    if (result.ok) { text.value = ""; notes.value = ""; setTimeout(() => { if (tab === "todo") showTab("todo"); }, 2500); }
  };
  const adding = element("div", "buttons");
  adding.append(add);
  const nodes = [element("h2", "", "Add a task"), labelControl("Project", picker), labelControl("Task", text),
                 labelControl("Notes (optional)", notes), adding, note];
  let shownProject = null;
  if (todos.length === 0) nodes.push(element("div", "empty", "Nothing to do. Add a task above, then start it in a session."));
  for (const item of [...todos].sort((a, b) => a.project.localeCompare(b.project))) {
    if (item.project !== shownProject) { shownProject = item.project; nodes.push(element("h2", "", item.project)); }
    const card = element("details", `card ${item.status === "doing" ? "working" : ""}`);
    const summary = element("summary");
    const row = element("span", "row");
    row.append(element("span", "project", `${"!".repeat(Math.max(0, item.priority - 2))} ${item.text}`.trim()));
    row.append(element("span", "state", item.status === "doing" ? "in progress" : ""));
    summary.append(row);
    card.append(summary);
    const details = element("div");
    if (item.notes) details.append(element("div", "notes", item.notes));
    const itemNote = element("div", "note");
    const agent = agentPicker(agentChoices);
    const start = element("button", "run", "Start in a new session");
    start.onclick = async (event) => {
      event.stopPropagation();
      const result = await post(`/api/todos/${item.id}/start`, { agent: agent.value }, itemNote);
      if (result.ok) setTimeout(() => { if (tab === "todo") showTab("todo"); }, 2500);
    };
    const buttons = element("div", "buttons");
    buttons.append(start);
    details.append(labelControl("Agent", agent), buttons, itemNote);
    card.append(details);
    nodes.push(card);
  }
  box.replaceChildren(...nodes);
}

const CI_LOOK = { failing: ["request", "✗ checks failing"], pending: ["task", "◌ checks running"], passing: ["outcome", "✓ checks pass"] };

async function renderPulls(box, isRefresh) {
  const { pulls, error, fetched_at: fetchedAt } = await getJson(isRefresh ? "/api/pulls?refresh=1" : "/api/pulls");
  const refreshing = element("button", "no", "Refresh");
  refreshing.onclick = () => { box.replaceChildren(element("div", "empty", "…")); renderPulls(box, true).catch((failure) => box.replaceChildren(element("div", "warning", String(failure)))); };
  const head = element("div", "buttons");
  head.append(refreshing);
  const nodes = [head];
  if (error) nodes.push(element("div", "warning", error));
  if (fetchedAt) nodes.push(element("div", "note", `fetched ${age(fetchedAt)} ago`));
  if (pulls.length === 0) nodes.push(element("div", "empty", "No open pull request."));
  for (const pull of pulls) {
    const card = element("div", "card");
    const row = element("div", "row");
    row.append(element("span", "project", `${pull.repository}#${pull.number}${pull.draft ? " · draft" : ""}`));
    row.append(element("span", "state", pull.updated));
    card.append(row, element("div", "task", pull.title));
    const [look, label] = CI_LOOK[pull.ci] || [null, null];
    if (label) card.append(element("div", look, label));
    if (pull.review) card.append(element("div", "task", pull.review.toLowerCase().replace(/_/g, " ")));
    card.append(element("div", "task", `by ${pull.author}`));
    // Only GitHub's own addresses become links.
    if (/^https:\/\/github\.com\//.test(pull.url)) {
      const link = element("a", "link", "Open on GitHub");
      link.href = pull.url;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      card.append(link);
    }
    nodes.push(card);
  }
  box.replaceChildren(...nodes);
}

function hours(seconds) {
  const minutes = Math.round((seconds || 0) / 60);
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h${String(minutes % 60).padStart(2, "0")}`;
}

async function renderUsage(box) {
  const usage = await getJson("/api/usage");
  const nodes = [element("h2", "", `Plan${usage.plan ? ` · ${usage.plan}` : ""}`)];
  if (usage.windows.length === 0) nodes.push(element("div", "empty", "No usage known yet: the cockpit checks it every 5 minutes."));
  for (const window of usage.windows) {
    const card = element("div", "card");
    const row = element("div", "row");
    row.append(element("span", "project", window.label));
    row.append(element("span", "state", window.percentage === null ? "?" : `${window.percentage}%`));
    const bar = element("div", "bar");
    const fill = element("div", "fill");
    const used = Math.min(100, Math.max(0, window.percentage || 0));
    fill.style.width = `${used}%`;
    fill.style.background = used >= 90 ? "var(--red)" : used >= 70 ? "var(--yellow)" : "var(--green)";
    bar.append(fill);
    card.append(row, bar);
    if (window.resets) card.append(element("div", "task", window.resets));
    if (window.note) card.append(element("div", "task", window.note));
    nodes.push(card);
  }
  if (usage.checked_at) nodes.push(element("div", "note", `checked ${age(usage.checked_at)} ago`));
  nodes.push(element("h2", "", "Cost estimate (API prices)"));
  if (usage.cost.error) nodes.push(element("div", "warning", usage.cost.error));
  else nodes.push(element("div", "card", `today $${usage.cost.today.toFixed(2)} · week $${usage.cost.week.toFixed(2)} · month $${usage.cost.month.toFixed(2)}`));
  if (usage.time) {
    nodes.push(element("h2", "", "Time with Claude"));
    nodes.push(element("div", "card", `today ${hours(usage.time.today)} · week ${hours(usage.time.week)} · month ${hours(usage.time.month)}`));
  }
  box.replaceChildren(...nodes);
}

function render() {
  if (!isPaired) return;
  if (opened !== null && !state.sessions.some((session) => session.session === opened)) {
    drafts.delete(opened);
    opened = null;
    const detail = document.getElementById("detail");
    detail.replaceChildren(); detail.dataset.session = "";
    shownScreen = "";
  }
  renderApprovals();
  if (tab !== "sessions") return;
  renderPhone();
  const detail = opened !== null;
  document.getElementById("view-title").textContent = detail ? state.sessions.find((session) => session.session === opened).project : "Sessions";
  document.getElementById("starter").hidden = detail;
  document.getElementById("list").hidden = detail;
  if (detail) { renderDetail(); loadScreen(); } else document.getElementById("detail").hidden = true;
  if (!detail) renderList();
}

// What the detail view's card and buttons depend on, to tell when they need redrawing.
function detailLook() {
  const session = state.sessions.find((s) => s.session === opened);
  return session ? JSON.stringify([session.state, session.request, session.detail, session.remote_control, session.task,
                                   session.needs_input, session.queued, session.features, state.approvals,
                                   (state.watched || []).includes(session.session), state.push]) : "";
}

async function refresh() {
  try {
    const response = await call("GET", "/api/state");
    if (response.status === 401) { isPaired = false; renderPairing(await response.text()); return; }
    if (!response.ok) throw new Error(await response.text());
    const before = detailLook();
    state = await response.json();
    const wasPaired = isPaired;
    isPaired = true;
    document.getElementById("updated").textContent = `Updated ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
    document.getElementById("updated").dataset.state = "online";
    if (!wasPaired) {
      document.getElementById("pairing").hidden = true;
      document.getElementById("tabs").hidden = false;
      render();
      renderStarter().catch((error) => {
        document.getElementById("starter").replaceChildren(element("div", "warning", String(error)));
      });
      return;
    }
    // The detail view redraws only when what it shows changed, keeping what is typed.
    renderApprovals();
    if (tab !== "sessions") return;
    renderPhone();
    if (opened === null) renderList();
    else if (detailLook() !== before) { render(); loadConversation(); loadScreen(); }
  } catch (error) {
    document.getElementById("updated").textContent = "offline";
    document.getElementById("updated").dataset.state = "offline";
  }
}

// A tap on a notification opens its session: in a new window through the address, in an open one
// through the service worker's message.
function openSession(session) {
  if (!session) return;
  if (tab !== "sessions") showTab("sessions");
  opened = session;
  render();
  loadConversation();
}
if (location.hash.startsWith("#session=")) opened = location.hash.slice("#session=".length);
navigator.serviceWorker?.addEventListener("message", (event) => openSession(event.data && event.data.session));
if (canPush) navigator.serviceWorker.register("/sw.js").catch(() => {});

renderTabs();
document.getElementById("refresh-view").onclick = () => {
  refresh();
  if (tab === "sessions") { if (opened) { loadConversation(); loadScreen(); } }
  else showTab(tab);
};
refresh();
setInterval(refresh, 3000);
setInterval(() => { if (opened) { loadConversation(); loadScreen(); } }, 1500);
if (window.visualViewport) {
  const fitKeyboard = () => document.documentElement.style.setProperty("--mobile-height", `${window.visualViewport.height}px`);
  window.visualViewport.addEventListener("resize", fitKeyboard);
  fitKeyboard();
}
