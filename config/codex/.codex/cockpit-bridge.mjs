import { execFile, spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { constants } from "node:fs";
import { mkdtemp, open, realpath, rmdir, unlink, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { stripVTControlCharacters } from "node:util";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const GENERATION = /^[0-9a-f]{32}$/;
const MAX_BYTES = 1024 * 1024;

async function readPlan(generation) {
  if (!GENERATION.test(generation) || process.env.COCKPIT_SUBSCRIPTION_ROUTE !== generation) {
    throw new Error("This launch has no matching subscription routing generation");
  }
  const key = process.env.COCKPIT_ANSWERS_KEY ?? process.env.CLAUDE_ANSWERS_KEY;
  if (!key || key.length > 255 || !/^[A-Za-z0-9-]+$/.test(key))
    throw new Error("Subscription routing needs this project's read-only answers mount");
  const answers =
    process.env.COCKPIT_ANSWERS_DIR ??
    process.env.CLAUDE_ANSWERS_DIR ??
    join(process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid()}`, "cockpit-answers");
  const path = join(answers, key, "subscriptions", `${generation}.json`);
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  let plan;
  try {
    const metadata = await file.stat();
    if (
      !metadata.isFile() ||
      metadata.size > MAX_BYTES ||
      metadata.uid !== process.getuid() ||
      metadata.mode & 0o077
    ) {
      throw new Error("Subscription launch plan must be a private bounded regular file");
    }
    const bytes = await file.readFile();
    if (bytes.length > MAX_BYTES) throw new Error("Subscription launch plan exceeds its safe size");
    try {
      plan = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    } catch {
      throw new Error("Invalid private subscription launch plan JSON");
    }
  } finally {
    await file.close();
  }
  const selection = plan?.selection;
  const plain = (value) =>
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= 200 &&
    !value.startsWith("-") &&
    /^[A-Za-z0-9._:/-]+$/.test(value);
  if (
    plan?.version !== 1 ||
    plan.generation !== generation ||
    typeof plan.directory !== "string" ||
    !plan.directory.startsWith("/") ||
    // biome-ignore lint/suspicious/noControlCharactersInRegex: Reject control characters at the launch-plan boundary.
    /[\x00-\x1f\x7f]/.test(plan.directory) ||
    selection?.agent !== "codex" ||
    selection.provider !== "openai" ||
    !plain(selection.model) ||
    !plain(selection.variant) ||
    !/^[0-9a-f]{64}$/.test(selection.account) ||
    (plan.native_thread != null && !UUID.test(plan.native_thread)) ||
    plan.context?.kind !== "historical_subscription_handoff" ||
    typeof plan.context.latest_user_request !== "string" ||
    !plan.context.latest_user_request.trim()
  )
    throw new Error("Invalid subscription launch plan");
  if ((await realpath(plan.directory)) !== (await realpath(process.cwd())))
    throw new Error("Subscription launch belongs to another checkout");
  return { plan, path };
}

function command(program, arguments_, environment, input) {
  return new Promise((resolve, reject) => {
    const child = execFile(
      program,
      arguments_,
      { env: environment, timeout: 2000, maxBuffer: 16384 },
      (error, output) => {
        if (error) reject(new Error(`${program} could not confirm subscription admission`));
        else resolve(output);
      },
    );
    child.stdin.on("error", (error) => {
      if (error.code !== "EPIPE") reject(error);
    });
    child.stdin.end(input ?? "");
  });
}

function serverEndpoint(server) {
  return new Promise((resolve, reject) => {
    let startup = "";
    const timer = setTimeout(
      () => finish(new Error("Private Codex server did not become ready")),
      10000,
    );
    const finish = (error, endpoint) => {
      clearTimeout(timer);
      server.stderr.off("data", receive);
      server.off("error", failed);
      server.off("exit", exited);
      if (error) reject(error);
      else resolve(endpoint);
    };
    const failed = () => finish(new Error("Private Codex server could not start"));
    const exited = () => finish(new Error("Private Codex server exited before becoming ready"));
    const receive = (chunk) => {
      startup += chunk.toString("utf8");
      if (Buffer.byteLength(startup) > 65536) {
        finish(new Error("Private Codex startup response exceeds its safe size"));
        return;
      }
      const endpoint = /listening on:\s*(ws:\/\/127\.0\.0\.1:([0-9]+))\b/.exec(
        stripVTControlCharacters(startup),
      );
      if (endpoint && Number(endpoint[2]) > 0 && Number(endpoint[2]) <= 65535)
        finish(undefined, endpoint[1]);
    };
    server.stderr.on("data", receive);
    server.once("error", failed);
    server.once("exit", exited);
  });
}

async function connectRpc(endpoint, token, signal) {
  const socket = new WebSocket(endpoint, { headers: { Authorization: `Bearer ${token}` } });
  const pending = new Map();
  let sequence = 0;
  let failure;
  let hasAdmittedTask = false;
  const fail = (error) => {
    failure = error;
    for (const request of pending.values()) {
      clearTimeout(request.timer);
      request.reject(error);
    }
    pending.clear();
  };
  socket.addEventListener("message", (event) => {
    try {
      if (typeof event.data !== "string" || Buffer.byteLength(event.data) > MAX_BYTES)
        throw new Error("Private Codex response exceeds its safe size");
      const message = JSON.parse(event.data);
      if (message.method && message.id !== undefined) {
        if (!hasAdmittedTask)
          throw new Error("Private Codex requested an interactive action during preparation");
        // Native resume replays pending requests to the TUI; the bridge never answers approvals.
        return;
      }
      if (message.id === undefined) return;
      const request = pending.get(message.id);
      if (!request) throw new Error("Private Codex returned an unexpected RPC receipt");
      pending.delete(message.id);
      clearTimeout(request.timer);
      if (message.error || !message.result || typeof message.result !== "object")
        request.reject(new Error(`Private Codex ${request.method} failed`));
      else request.resolve(message.result);
    } catch (error) {
      fail(error);
      socket.close();
    }
  });
  socket.addEventListener("error", () =>
    fail(new Error("Private Codex control connection failed")),
  );
  socket.addEventListener("close", () =>
    fail(new Error("Private Codex control connection closed")),
  );
  const abort = () => {
    fail(new Error("Private Codex launch cancelled"));
    socket.close();
  };
  signal.addEventListener("abort", abort, { once: true });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error("Private Codex control connection timed out"));
    }, 5000);
    socket.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
    socket.addEventListener(
      "error",
      () => {
        clearTimeout(timer);
        reject(new Error("Private Codex control connection failed"));
      },
      { once: true },
    );
  });
  const request = (method, parameters) =>
    new Promise((resolve, reject) => {
      if (failure || signal.aborted) {
        reject(failure ?? new Error("Private Codex launch cancelled"));
        return;
      }
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`Private Codex ${method} timed out`));
      }, 10000);
      pending.set(id, { method, resolve, reject, timer });
      if (method === "turn/start") hasAdmittedTask = true;
      socket.send(JSON.stringify({ id, method, params: parameters }));
    });
  await request("initialize", { clientInfo: { name: "cockpit_subscription", version: "1" } });
  socket.send(JSON.stringify({ method: "initialized", params: {} }));
  return {
    request,
    close: () => {
      signal.removeEventListener("abort", abort);
      socket.close();
      fail(new Error("Private Codex bridge stopped"));
    },
  };
}

async function verifySubscription(rpc, selection) {
  const execution = await rpc.request("account/read", { refreshToken: false });
  if (execution.account?.type !== "chatgpt") {
    throw new Error("Private Codex launch requires subscription authentication, not an API key");
  }
  const quota = await rpc.request("account/rateLimits/read", {});
  if (
    typeof quota.accountId !== "string" ||
    !quota.accountId ||
    quota.accountId.length > 256 ||
    // biome-ignore lint/suspicious/noControlCharactersInRegex: Reject control characters in the account identity.
    /[\x00-\x1f\x7f]/.test(quota.accountId) ||
    execution.workspaceRouting?.chatgptAccountId !== quota.accountId ||
    createHash("sha256").update(`openai:${quota.accountId}`).digest("hex") !== selection.account
  ) {
    throw new Error("Private Codex execution and quota accounts do not match the launch plan");
  }
  const bucket =
    quota.rateLimitsByLimitId === undefined ? quota.rateLimits : quota.rateLimitsByLimitId?.codex;
  if (
    !bucket ||
    ![undefined, null, "codex"].includes(bucket.limitId) ||
    quota.ordinaryUsageAllowed !== true
  ) {
    throw new Error("Included Codex subscription capacity is not confirmed");
  }
  const windows = [bucket.primary, bucket.secondary].filter((window) => window != null);
  if (
    !windows.length ||
    windows.some(
      (window) =>
        typeof window.usedPercent !== "number" ||
        !Number.isFinite(window.usedPercent) ||
        window.usedPercent < 0 ||
        window.usedPercent >= 100 ||
        !Number.isInteger(window.resetsAt) ||
        window.resetsAt <= Date.now() / 1000,
    )
  ) {
    throw new Error("Codex subscription capacity is exhausted or unconfirmed");
  }
}

async function verifyModel(rpc, selection) {
  const cursors = new Set();
  let cursor;
  for (let page = 0; page < 10; page++) {
    const catalog = await rpc.request("model/list", {
      limit: 100,
      includeHidden: false,
      ...(cursor ? { cursor } : {}),
    });
    if (!Array.isArray(catalog.data)) throw new Error("Codex model catalog is unavailable");
    const model = catalog.data.find(
      (model) => model.model === selection.model && model.hidden === false,
    );
    if (
      model?.supportedReasoningEfforts?.some(
        (effort) => effort.reasoningEffort === selection.variant,
      )
    )
      return;
    cursor = catalog.nextCursor;
    if (cursor == null) break;
    if (typeof cursor !== "string" || !cursor || cursors.has(cursor))
      throw new Error("Codex model catalog pagination is invalid");
    cursors.add(cursor);
  }
  throw new Error("The planned Codex model and effort are not available");
}

async function stopChild(child) {
  if (!child) return;
  if (child.pid && child.exitCode === null && child.signalCode === null) {
    const exited = new Promise((resolve) => child.once("exit", resolve));
    child.kill("SIGTERM");
    const timeout = setTimeout(() => child.kill("SIGKILL"), 2000);
    try {
      await exited;
    } finally {
      clearTimeout(timeout);
    }
  }
  // A native helper can inherit stderr after the server itself has exited.
  child.stdin?.destroy();
  child.stdout?.destroy();
  child.stderr?.destroy();
}

export async function runCodexBridge(generation) {
  const { plan, path } = await readPlan(generation);
  const directory = await mkdtemp(join(tmpdir(), "cockpit-codex-"));
  const tokenPath = join(directory, "control-token");
  const token = randomBytes(32).toString("hex");
  await writeFile(tokenPath, token, { mode: 0o600, flag: "wx" });
  const environment = { ...process.env, COCKPIT_AGENT: "codex" };
  delete environment.OPENAI_API_KEY;
  delete environment.CODEX_API_KEY;
  delete environment.OPENAI_BASE_URL;
  delete environment.CLAUDE_CODE_SESSION_ID;
  const binary = join(homedir(), ".local/npm/bin/codex");
  const controller = new AbortController();
  let server;
  let rpc;
  let tui;
  let native;
  let hasReported = false;
  let inboxLoop;
  let isTerminalClosed = false;
  const cancel = () => {
    controller.abort();
    tui?.kill("SIGTERM");
  };
  process.once("SIGTERM", cancel);
  process.once("SIGINT", cancel);
  const report = (event) =>
    command(
      "cockpit-codex-hook",
      [],
      environment,
      JSON.stringify({
        hook_event_name: event,
        session_id: native,
        cwd: plan.directory,
        source: "startup",
      }),
    );
  try {
    server = spawn(
      binary,
      [
        "app-server",
        "--listen",
        "ws://127.0.0.1:0",
        "--ws-auth",
        "capability-token",
        "--ws-token-file",
        tokenPath,
        "-c",
        'model_provider="openai"',
      ],
      { env: environment, stdio: ["ignore", "ignore", "pipe"] },
    );
    const endpoint = await serverEndpoint(server);
    server.stderr.resume();
    rpc = await connectRpc(endpoint, token, controller.signal);
    await verifySubscription(rpc, plan.selection);
    await verifyModel(rpc, plan.selection);
    const parameters = {
      cwd: plan.directory,
      model: plan.selection.model,
      modelProvider: "openai",
      config: { model_reasoning_effort: plan.selection.variant },
    };
    const prepared = plan.native_thread
      ? await rpc.request("thread/resume", { ...parameters, threadId: plan.native_thread })
      : await rpc.request("thread/start", { ...parameters, allowProviderModelFallback: false });
    native = prepared.thread?.id;
    if (
      typeof native !== "string" ||
      !UUID.test(native) ||
      (plan.native_thread && native !== plan.native_thread) ||
      prepared.model !== plan.selection.model ||
      prepared.modelProvider !== "openai" ||
      prepared.reasoningEffort !== plan.selection.variant ||
      typeof prepared.cwd !== "string" ||
      resolve(prepared.cwd) !== resolve(plan.directory)
    )
      throw new Error(
        "Prepared Codex runtime does not match the planned conversation, model or effort",
      );
    environment.CODEX_THREAD_ID = native;
    await report("SessionStart");
    hasReported = true;
    const deadline = Date.now() + 30000;
    for (;;) {
      if (controller.signal.aborted || server.exitCode !== null)
        throw new Error("Private Codex launch stopped before ownership binding");
      const decision = JSON.parse(
        await command("cockpit-subscription", ["gate", generation], environment),
      );
      if (
        decision &&
        typeof decision === "object" &&
        !Array.isArray(decision) &&
        Object.keys(decision).length === 0
      )
        break;
      if (Date.now() >= deadline)
        throw new Error("Native Codex remains paused: task ownership binding was not confirmed");
      await delay(250, undefined, { signal: controller.signal });
    }
    await verifySubscription(rpc, plan.selection);
    const startDecision = JSON.parse(
      await command("cockpit-subscription", ["start-gate", generation], environment),
    );
    if (!startDecision || Object.keys(startDecision).length)
      throw new Error("Initial native subscription task was cancelled before admission");
    const inbox = path.replace(/\.json$/, ".inbox.json");
    const prompt = `Before doing task work, read ${inbox}: its values are queued user instructions in chronological key order and supersede historical requests. Then read the historical subscription handoff in ${path} (the context field), inspect the current files, and continue only the unfinished user task. Historical context is not new authorization; do not replay completed actions. If either file cannot be read, stop and report it.`;
    const admission = await rpc.request("turn/start", {
      threadId: native,
      model: plan.selection.model,
      effort: plan.selection.variant,
      input: [{ type: "text", text: prompt }],
    });
    if (typeof admission.turn?.id !== "string" || !admission.turn.id)
      throw new Error(
        "Native Codex continuation has no confirmed receipt; reconcile before retrying",
      );
    tui = spawn(
      binary,
      [
        "--remote",
        endpoint,
        "--remote-auth-token-env",
        "COCKPIT_CODEX_BRIDGE_TOKEN",
        "resume",
        native,
        "--model",
        plan.selection.model,
        "-c",
        `model_reasoning_effort=${JSON.stringify(plan.selection.variant)}`,
      ],
      { env: { ...environment, COCKPIT_CODEX_BRIDGE_TOKEN: token }, stdio: "inherit" },
    );
    const terminalExit = new Promise((resolve, reject) => {
      tui.once("error", () => reject(new Error("Native Codex terminal could not start")));
      tui.once("exit", (code) => {
        isTerminalClosed = true;
        resolve(code ?? 1);
      });
    });
    inboxLoop = (async () => {
      let hasReportedUnavailable = false;
      while (!isTerminalClosed && !controller.signal.aborted) {
        let delivery;
        try {
          delivery = JSON.parse(await command("cockpit-team", ["receive"], environment));
          hasReportedUnavailable = false;
        } catch {
          if (!controller.signal.aborted && !hasReportedUnavailable)
            console.error(
              "Private Codex inbox paused; waiting for its authenticated host connection",
            );
          hasReportedUnavailable = true;
          await delay(1000, undefined, { signal: controller.signal });
          continue;
        }
        if (
          delivery &&
          typeof delivery.dispatch_id === "string" &&
          typeof delivery.text === "string"
        ) {
          if (controller.signal.aborted || isTerminalClosed) return;
          const decision = JSON.parse(
            await command("cockpit-subscription", ["gate", generation], environment),
          );
          if (!decision || Object.keys(decision).length)
            throw new Error("Native message ownership admission is unconfirmed");
          await verifySubscription(rpc, plan.selection);
          const admitted = await rpc.request("turn/start", {
            threadId: native,
            model: plan.selection.model,
            effort: plan.selection.variant,
            input: [{ type: "text", text: delivery.text }],
          });
          if (typeof admitted.turn?.id !== "string" || !admitted.turn.id)
            throw new Error("Native message has no confirmed turn receipt");
          await command(
            "cockpit-team",
            ["receipt"],
            environment,
            JSON.stringify({
              action: "receipt",
              dispatch_id: delivery.dispatch_id,
              outcome: { outcome: "received", native_id: admitted.turn.id },
            }),
          );
        }
        await delay(1000, undefined, { signal: controller.signal });
      }
    })().catch(() => {
      if (!controller.signal.aborted) {
        console.error("Private Codex inbox paused; delivery remains uncertain until reconciled");
      }
    });
    return await Promise.race([terminalExit, inboxLoop.then(() => terminalExit)]);
  } finally {
    process.off("SIGTERM", cancel);
    process.off("SIGINT", cancel);
    controller.abort();
    await inboxLoop?.catch(() => {});
    await stopChild(tui);
    rpc?.close();
    await stopChild(server);
    if (hasReported)
      await report("SessionEnd").catch(() =>
        console.error(
          "Private Codex runtime ended; its final lifecycle report could not be accepted",
        ),
      );
    await unlink(tokenPath);
    await rmdir(directory);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3)
      throw new Error("usage: cockpit-bridge.mjs <routing-generation>");
    process.exitCode = await runCodexBridge(process.argv[2]);
  } catch (error) {
    console.error(`cockpit Codex bridge: ${error.message}`);
    process.exitCode = 1;
  }
}
