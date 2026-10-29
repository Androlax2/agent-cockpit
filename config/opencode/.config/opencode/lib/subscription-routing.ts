import { createHash, randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { homedir } from "node:os";

type Model = { providerID: string; id: string; variant?: string };
type Capacity = "available" | "exhausted" | "unknown";
type Owner = { model: Model; connection: string; account?: string };
type Route = {
  version: 1;
  original: Owner;
  current: Owner;
  phase: "waiting" | "switching" | "admitting" | "active" | "uncertain";
  pendingFailure: boolean;
  lastFailure?: string;
  target?: Owner;
  inboxID?: string;
  resume?: boolean;
};

const keyOf = (model: Model) => JSON.stringify([model.providerID, model.id, model.variant ?? null]);
const isModel = (model: any): model is Model =>
  !!model &&
  ["anthropic", "openai"].includes(model.providerID) &&
  typeof model.id === "string" &&
  model.id.length > 0 &&
  model.id.length <= 200 &&
  // biome-ignore lint/suspicious/noControlCharactersInRegex: Model identifiers must reject controls.
  !/[\s#\x00-\x1f]/.test(model.id) &&
  (model.variant === undefined ||
    (typeof model.variant === "string" &&
      model.variant.length > 0 &&
      model.variant.length <= 100 &&
      // biome-ignore lint/suspicious/noControlCharactersInRegex: Variant identifiers must reject controls.
      !/[\s#\x00-\x1f]/.test(model.variant)));

const isOwner = (owner: any): owner is Owner =>
  isModel(owner?.model) &&
  typeof owner.connection === "string" &&
  owner.connection.length > 0 &&
  owner.connection.length <= 200 &&
  (owner.model.providerID !== "openai" ||
    (typeof owner.account === "string" && /^[0-9a-f]{64}$/.test(owner.account)));

const accountOf = (credential: any) =>
  typeof credential?.metadata?.accountID === "string" && credential.metadata.accountID.length > 0
    ? createHash("sha256").update(`openai:${credential.metadata.accountID}`).digest("hex")
    : undefined;

const anthropicAccount = (profile: any) =>
  [profile?.organization?.uuid, profile?.account?.uuid].every(
    (value) => typeof value === "string" && value.length > 0 && value.length <= 256,
  )
    ? createHash("sha256")
        .update(`anthropic:${profile.organization.uuid}:${profile.account.uuid}`)
        .digest("hex")
    : undefined;

const sameConnection = (left: Owner | undefined, right: Owner) =>
  !!left &&
  left.connection === right.connection &&
  left.model.providerID === right.model.providerID &&
  left.account === right.account;

function validRoute(route: any): route is Route {
  return (
    route?.version === 1 &&
    isOwner(route.original) &&
    isOwner(route.current) &&
    ["waiting", "switching", "admitting", "active", "uncertain"].includes(route.phase) &&
    typeof route.pendingFailure === "boolean" &&
    (!route.target || isOwner(route.target)) &&
    (!["switching", "admitting", "uncertain"].includes(route.phase) ||
      (isOwner(route.target) &&
        /^msg_cockpitquota[0-9a-f]{32}$/.test(route.inboxID) &&
        typeof route.resume === "boolean"))
  );
}

export function subscriptionCapacity(
  provider: string,
  model: string,
  usage: any,
  now: number,
): Capacity {
  const classify = (used: unknown, reset: number): Capacity =>
    typeof used !== "number" ||
    !Number.isFinite(used) ||
    used < 0 ||
    used > 100 ||
    !Number.isFinite(reset) ||
    reset <= now
      ? "unknown"
      : used === 100
        ? "exhausted"
        : "available";
  const states: Capacity[] = [];
  if (provider === "openai") {
    if (typeof usage?.rate_limit?.allowed !== "boolean") return "unknown";
    for (const key of ["primary_window", "secondary_window"]) {
      const window = usage.rate_limit[key];
      if (window != null)
        states.push(
          classify(
            window.used_percent,
            Number.isInteger(window.reset_at) ? window.reset_at * 1000 : NaN,
          ),
        );
    }
    if (!states.length || states.includes("unknown")) return "unknown";
    if (!usage.rate_limit.allowed || usage.rate_limit.limit_reached === true) return "exhausted";
  } else if (provider === "anthropic") {
    if (usage?.limits !== undefined && !Array.isArray(usage.limits)) return "unknown";
    const family = /^claude-(opus|sonnet|haiku)-/.exec(model)?.[1];
    if (!family) return "unknown";
    const window = (value: any, percent: string) =>
      classify(value?.[percent], Date.parse(value?.resets_at));
    const scoped = (name: unknown) =>
      typeof name === "string"
        ? ["opus", "sonnet", "haiku"].find(
            (family) =>
              name.toLowerCase() === family || name.toLowerCase().startsWith(`claude ${family}`),
          )
        : undefined;
    if (Array.isArray(usage?.limits)) {
      if (
        usage.limits.some(
          (limit: any) =>
            ["session", "weekly_all"].includes(limit?.kind) &&
            window(limit, "percent") === "exhausted",
        )
      )
        return "exhausted";
      for (const limit of usage.limits) {
        if (limit.kind === "session" || limit.kind === "weekly_all")
          states.push(window(limit, "percent"));
        else if (limit.kind === "weekly_scoped") {
          const scope = scoped(limit.scope?.model?.display_name);
          if (!scope) return "unknown";
          if (scope === family) states.push(window(limit, "percent"));
        } else return "unknown";
      }
    } else {
      for (const key of ["five_hour", "seven_day", `seven_day_${family}`]) {
        if (usage?.[key] != null) states.push(window(usage[key], "utilization"));
      }
      if (usage?.model_scoped != null) {
        if (!Array.isArray(usage.model_scoped)) return "unknown";
        for (const limit of usage.model_scoped) {
          const scope = scoped(limit.display_name);
          if (!scope) return "unknown";
          if (scope === family) states.push(window(limit, "utilization"));
        }
      }
    }
  } else return "unknown";
  return !states.length || states.includes("unknown")
    ? "unknown"
    : states.includes("exhausted")
      ? "exhausted"
      : "available";
}

export async function setupSubscriptionRouting(ctx: any, sessionID: string) {
  if (!ctx.storage || !ctx.integration?.connection || !ctx.session?.hook) {
    console.warn(
      "cockpit plugin: subscription fallback requires the OpenCode V2 storage, connection and session APIs",
    );
    return { handle: async (_event: any) => {}, close: () => {} };
  }
  const storageKey = `subscription-route/${sessionID}`;
  const seenKey = `subscription-models/${sessionID}`;
  const formsKey = `subscription-forms/${sessionID}`;
  const savedForms = (await ctx.storage.get(formsKey)) ?? [];
  if (
    !Array.isArray(savedForms) ||
    savedForms.some((form) => typeof form?.id !== "string" || typeof form?.sessionID !== "string")
  ) {
    throw new Error("invalid saved subscription form blockers; preserve them before reconciling");
  }
  const forms = new Map<string, string>(savedForms.map((form) => [form.id, form.sessionID]));
  const controller = new AbortController();
  let route: Route | undefined = await ctx.storage.get(storageKey);
  let stopped = false;
  let busy = false;
  let inputVersion = 0;
  if (route && !validRoute(route)) {
    throw new Error(
      "invalid saved subscription route; preserve it and reconcile before continuing",
    );
  }
  const save = async () => {
    if (stopped) return;
    if (route) await ctx.storage.set(storageKey, JSON.parse(JSON.stringify(route)));
    else await ctx.storage.remove(storageKey);
  };
  const owner = async (model: Model): Promise<Owner | undefined> => {
    const connection = await ctx.integration.connection.active(model.providerID);
    if (
      connection?.type !== "credential" ||
      connection.status?.status === "needs_auth" ||
      typeof connection.id !== "string" ||
      !connection.id ||
      connection.id.length > 200
    )
      return;
    const credential = await ctx.integration.connection.resolve(connection);
    if (credential?.type !== "oauth") return;
    let account = model.providerID === "openai" ? accountOf(credential) : undefined;
    if (model.providerID === "anthropic") {
      try {
        account = anthropicAccount(await claudeProfile(credential));
        const current = await ctx.integration.connection.active("anthropic");
        if (
          current?.id !== connection.id ||
          (await ctx.integration.connection.resolve(current))?.access !== credential.access
        )
          return;
      } catch {
        return;
      }
    }
    if (!account) return;
    return { model: { ...model }, connection: connection.id, ...(account ? { account } : {}) };
  };
  const claudeProfile = async (credential: any) => {
    if (
      typeof credential.access !== "string" ||
      !credential.access ||
      !Number.isFinite(credential.expires) ||
      credential.expires <= Date.now()
    ) {
      throw new Error("Claude authentication is unconfirmed");
    }
    const response = await fetch("https://api.anthropic.com/api/oauth/profile", {
      headers: {
        Authorization: `Bearer ${credential.access}`,
        Accept: "application/json",
        "anthropic-beta": "oauth-2025-04-20",
      },
      signal: AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]),
      redirect: "error",
    });
    if (!response.ok || !response.body) throw new Error("profile unavailable");
    const chunks: Uint8Array[] = [];
    let bytes = 0;
    for await (const chunk of response.body) {
      bytes += chunk.length;
      if (bytes > 1024 * 1024) throw new Error("profile exceeds the safe size");
      chunks.push(chunk);
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  };
  const billingEligible = async (selected: Owner): Promise<boolean> => {
    if (selected.model.providerID !== "anthropic") return true;
    try {
      const connection = await ctx.integration.connection.active("anthropic");
      if (connection?.id !== selected.connection || connection.type !== "credential") return false;
      const credential = await ctx.integration.connection.resolve(connection);
      if (
        credential?.type !== "oauth" ||
        typeof credential.access !== "string" ||
        !credential.access ||
        !Number.isFinite(credential.expires) ||
        credential.expires <= Date.now()
      )
        return false;
      const profile = await claudeProfile(credential);
      const current = await ctx.integration.connection.active("anthropic");
      if (stopped || current?.id !== selected.connection || current.type !== "credential")
        return false;
      const resolved = await ctx.integration.connection.resolve(current);
      return (
        resolved?.type === "oauth" &&
        resolved.access === credential.access &&
        anthropicAccount(profile) === selected.account &&
        profile?.organization?.has_extra_usage_enabled === false
      );
    } catch {
      console.warn(
        "cockpit plugin: cannot verify Claude extra usage is disabled; automatic Claude routing paused",
      );
      return false;
    }
  };
  const quota = async (selected: Owner, force = false): Promise<Capacity> => {
    const connection = await ctx.integration.connection.active(selected.model.providerID);
    if (connection?.id !== selected.connection || connection.type !== "credential")
      return "unknown";
    const credential = await ctx.integration.connection.resolve(connection);
    if (
      credential?.type !== "oauth" ||
      typeof credential.access !== "string" ||
      !credential.access ||
      !Number.isFinite(credential.expires) ||
      credential.expires <= Date.now()
    )
      return "unknown";
    if (selected.model.providerID === "openai" && accountOf(credential) !== selected.account)
      return "unknown";
    if (
      selected.model.providerID === "anthropic" &&
      !sameConnection(await owner(selected.model), selected)
    )
      return "unknown";
    const cacheKey = `subscription-quota/${selected.model.providerID}/${selected.connection}`;
    const cached = await ctx.storage.get(cacheKey);
    const now = Date.now();
    if (
      !force &&
      cached &&
      Number.isFinite(cached.fetched) &&
      cached.fetched <= now &&
      now - cached.fetched < 300000 &&
      cached.account === selected.account
    ) {
      if (cached.failed || cached.matched !== true) return "unknown";
      const usage = cached.usage;
      const resets =
        selected.model.providerID === "openai"
          ? [usage?.rate_limit?.primary_window, usage?.rate_limit?.secondary_window].map(
              (window) => (typeof window?.reset_at === "number" ? window.reset_at * 1000 : NaN),
            )
          : [
              ...Object.values(usage ?? {}).filter(
                (value) => value && typeof value === "object" && !Array.isArray(value),
              ),
              ...(Array.isArray(usage?.limits) ? usage.limits : []),
              ...(Array.isArray(usage?.model_scoped) ? usage.model_scoped : []),
            ].map((window: any) => Date.parse(window.resets_at));
      if (!resets.some((reset) => reset > cached.fetched && reset <= now)) {
        return subscriptionCapacity(selected.model.providerID, selected.model.id, usage, now);
      }
    }
    try {
      const headers: Record<string, string> = {
        Authorization: `Bearer ${credential.access}`,
        Accept: "application/json",
      };
      let url: string;
      if (selected.model.providerID === "anthropic") {
        url = "https://api.anthropic.com/api/oauth/usage";
        headers["anthropic-beta"] = "oauth-2025-04-20";
      } else {
        const account = credential.metadata?.accountID;
        if (
          typeof account !== "string" ||
          !account ||
          account.length > 256 ||
          /[\r\n]/.test(account)
        )
          return "unknown";
        url = "https://chatgpt.com/backend-api/wham/usage";
        headers["ChatGPT-Account-Id"] = account;
      }
      const signal = AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]);
      const response = await fetch(url, { headers, signal, redirect: "error" });
      if (!response.ok || !response.body) throw new Error("quota unavailable");
      const chunks: Uint8Array[] = [];
      let bytes = 0;
      for await (const chunk of response.body) {
        bytes += chunk.length;
        if (bytes > 1024 * 1024) throw new Error("quota response exceeds the safe size");
        chunks.push(chunk);
      }
      const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const current = await ctx.integration.connection.active(selected.model.providerID);
      if (stopped || current?.id !== selected.connection) return "unknown";
      const matched =
        selected.model.providerID === "anthropic"
          ? sameConnection(await owner(selected.model), selected)
          : body.account_id === credential.metadata.accountID;
      const usage =
        selected.model.providerID === "openai"
          ? { rate_limit: body.rate_limit }
          : {
              five_hour: body.five_hour,
              seven_day: body.seven_day,
              seven_day_opus: body.seven_day_opus,
              seven_day_sonnet: body.seven_day_sonnet,
              seven_day_haiku: body.seven_day_haiku,
              limits: body.limits,
              model_scoped: body.model_scoped,
            };
      await ctx.storage.set(cacheKey, {
        fetched: Date.now(),
        matched,
        usage: JSON.parse(JSON.stringify(usage)),
        ...(selected.account ? { account: selected.account } : {}),
      });
      return matched
        ? subscriptionCapacity(selected.model.providerID, selected.model.id, usage, Date.now())
        : "unknown";
    } catch {
      if (!stopped) {
        await ctx.storage.set(cacheKey, {
          fetched: now,
          failed: true,
          ...(selected.account ? { account: selected.account } : {}),
        });
        console.warn(
          "cockpit plugin: subscription quota unavailable; leaving the session unchanged",
        );
      }
      return "unknown";
    }
  };
  const cancelManualChange = async () => {
    if (!route) return false;
    const info = await ctx.session.get({ sessionID });
    const expected =
      route.phase === "switching"
        ? ([route.current.model, route.target?.model].filter(Boolean) as Model[])
        : [route.current.model];
    if (
      !isModel(info.model) ||
      !expected.some((model) => keyOf(info.model) === keyOf(model)) ||
      !sameConnection(await owner(route.current.model), route.current) ||
      !sameConnection(await owner(route.original.model), route.original)
    ) {
      route = undefined;
      await save();
      console.info(
        "cockpit plugin: manual model or account change cancelled automatic subscription return",
      );
      return true;
    }
    return false;
  };
  const admit = async () => {
    if (!route?.target || !route.inboxID || stopped) return;
    if (!(await billingEligible(route.target))) return;
    if (!sameConnection(await owner(route.target.model), route.target)) {
      route = undefined;
      await save();
      return;
    }
    route.current = route.target;
    if (!route.resume) {
      route = undefined;
      await save();
      return;
    }
    route.phase = "admitting";
    await save();
    try {
      await ctx.session.synthetic({
        sessionID,
        id: route.inboxID,
        text: "Continue the task interrupted by the subscription limit using the existing conversation and current files. Do not replay completed actions. This changes neither the task scope nor tool or git authorization.",
        description: "Subscription capacity recovered; continue the interrupted task",
        metadata: { cockpit_subscription: { id: route.inboxID } },
        delivery: "queue",
        resume: true,
      });
      if (!route || stopped) return;
      route.phase = "active";
      route.pendingFailure = false;
      await save();
    } catch {
      if (route && !stopped) {
        route.phase = "uncertain";
        await save();
        console.warn(
          "cockpit plugin: subscription continuation has no receipt; reconcile the native inbox before retrying",
        );
      }
    }
  };
  const switchTo = async (target: Owner, resume: boolean) => {
    if (!route || stopped) return;
    const requestedInput = inputVersion;
    const models = await ctx.model.list();
    if (
      !models.some(
        (model: any) =>
          model.enabled &&
          model.capabilities?.tools &&
          model.providerID === target.model.providerID &&
          model.id === target.model.id &&
          (!target.model.variant ||
            model.variants?.some((variant: any) => variant.id === target.model.variant)),
      ) ||
      !sameConnection(await owner(target.model), target) ||
      !(await billingEligible(target))
    )
      return;
    if (!route || stopped || inputVersion !== requestedInput) return;
    route.target = target;
    route.inboxID = `msg_cockpitquota${randomBytes(16).toString("hex")}`;
    route.resume = resume;
    route.phase = "switching";
    await save();
    if (!route || stopped || inputVersion !== requestedInput) return;
    await ctx.session.switchModel({ sessionID, model: target.model });
    const info = await ctx.session.get({ sessionID });
    if (!route || stopped) return;
    if (!isModel(info.model) || keyOf(info.model) !== keyOf(target.model)) {
      route = undefined;
      await save();
      return;
    }
    await admit();
  };
  const waitBoundary = async (id: string, seen = new Set<string>()): Promise<boolean> => {
    if (seen.has(id) || seen.size >= 100 || stopped) return false;
    seen.add(id);
    if ([...forms.values()].includes(id)) return false;
    await ctx.session.wait({ sessionID: id }, { signal: controller.signal });
    if (
      stopped ||
      [...forms.values()].includes(id) ||
      (await ctx.permission.list({ sessionID: id })).length
    )
      return false;
    const history = await ctx.session.context({ sessionID: id });
    for (const message of history) {
      if (message.type === "shell" && message.status === "running") return false;
      for (const part of message.type === "assistant" ? (message.content ?? []) : []) {
        if (part.type !== "tool") continue;
        if (["streaming", "running"].includes(part.state?.status)) return false;
        if (part.name !== "subagent" || part.state?.metadata?.status !== "running") continue;
        const childID = part.state.metadata.sessionID;
        if (typeof childID !== "string") return false;
        if (seen.has(childID)) continue;
        const child = await ctx.session.get({ sessionID: childID });
        if (child.parentID !== id || !(await waitBoundary(childID, seen))) return false;
        if (!(await ctx.session.get({ sessionID: childID })).outcome) return false;
      }
    }
    return true;
  };
  const recover = async (force = false) => {
    if (stopped || busy || !route) return;
    busy = true;
    const interruptedInput = inputVersion;
    try {
      if ((await cancelManualChange()) || !route) return;
      if (route.phase === "admitting" || route.phase === "uncertain") {
        const history = await ctx.session.context({ sessionID });
        if (
          history.some(
            (message: any) =>
              message.id === route?.inboxID ||
              message.metadata?.cockpit_subscription?.id === route?.inboxID,
          )
        ) {
          route.phase = "active";
          route.pendingFailure = false;
          await save();
        }
        return;
      }
      if (route.phase === "switching") {
        const info = await ctx.session.get({ sessionID });
        if (route.target && keyOf(info.model) === keyOf(route.target.model)) await admit();
        return;
      }
      let originalCapacity = await quota(route.original, force);
      const currentCapacity =
        keyOf(route.current.model) === keyOf(route.original.model)
          ? originalCapacity
          : await quota(route.current, force);
      if (!route.pendingFailure && originalCapacity !== "available") return;
      if (
        route.pendingFailure &&
        currentCapacity !== "exhausted" &&
        currentCapacity !== "available"
      )
        return;
      if (!(await waitBoundary(sessionID))) return;
      if (stopped || inputVersion !== interruptedInput || (await cancelManualChange()) || !route)
        return;
      if (existsSync(`${homedir()}/.cache/claude/input-${sessionID}.question`)) return;
      originalCapacity = await quota(route.original);
      const info = await ctx.session.get({ sessionID });
      if (info.outcome === "succeeded") route.pendingFailure = false;
      if (originalCapacity === "available") {
        await switchTo(route.original, route.pendingFailure);
        return;
      }
      if (!route.pendingFailure) return;
      const seen = (await ctx.storage.get(seenKey)) ?? {};
      const models = await ctx.model.list();
      const candidates = models.filter(
        (model: any) =>
          model.enabled &&
          model.capabilities?.tools &&
          ["anthropic", "openai"].includes(model.providerID) &&
          model.providerID !== route!.current.model.providerID,
      );
      candidates.sort(
        (left: any, right: any) =>
          Number(seen[right.providerID]?.id === right.id) -
          Number(seen[left.providerID]?.id === left.id),
      );
      for (const model of candidates) {
        const saved = seen[model.providerID];
        const ref: Model =
          saved?.id === model.id &&
          (!saved.variant || model.variants?.some((variant: any) => variant.id === saved.variant))
            ? saved
            : { providerID: model.providerID, id: model.id };
        const target = await owner(ref);
        if (target && (await quota(target)) === "available") {
          await switchTo(target, true);
          return;
        }
      }
      route.phase = "waiting";
      await save();
    } finally {
      busy = false;
    }
  };
  const handle = async (event: any) => {
    if (stopped) return;
    if (
      event.type === "form.created" &&
      typeof event.data?.form?.id === "string" &&
      typeof event.data.form.sessionID === "string"
    ) {
      forms.set(event.data.form.id, event.data.form.sessionID);
      await ctx.storage.set(
        formsKey,
        [...forms].map(([id, sessionID]) => ({ id, sessionID })),
      );
      return;
    }
    if (
      ["form.replied", "form.cancelled"].includes(event.type) &&
      forms.get(event.data?.id) === event.data?.sessionID
    ) {
      forms.delete(event.data.id);
      await ctx.storage.set(
        formsKey,
        [...forms].map(([id, sessionID]) => ({ id, sessionID })),
      );
      return;
    }
    if (stopped || event.data?.sessionID !== sessionID) return;
    if (event.type === "session.execution.interrupted") {
      inputVersion++;
      route = undefined;
      await save();
      return;
    }
    if (event.type === "session.deleted") {
      route = undefined;
      await save();
      stopped = true;
      controller.abort();
      clearInterval(timer);
      return;
    }
    if (event.type === "session.model.selected" && isModel(event.data.model)) {
      const seen = (await ctx.storage.get(seenKey)) ?? {};
      seen[event.data.model.providerID] = event.data.model;
      await ctx.storage.set(seenKey, seen);
      if (
        route &&
        keyOf(event.data.model) !== keyOf(route.current.model) &&
        !(
          route.phase === "switching" &&
          route.target &&
          keyOf(event.data.model) === keyOf(route.target.model)
        )
      ) {
        route = undefined;
        await save();
      }
      return;
    }
    if (event.type === "session.execution.failed" && event.data.error?.status === 429) {
      const failedInput = inputVersion;
      if (event.id && route?.lastFailure === event.id) return;
      const info = await ctx.session.get({ sessionID });
      if (!isModel(info.model)) return;
      const selected = await owner(info.model);
      if (!selected || (await quota(selected, true)) !== "exhausted") return;
      if (stopped || inputVersion !== failedInput) return;
      if (!route)
        route = {
          version: 1,
          original: selected,
          current: selected,
          phase: "waiting",
          pendingFailure: true,
        };
      route.pendingFailure = true;
      route.lastFailure = event.id;
      const seen = (await ctx.storage.get(seenKey)) ?? {};
      seen[info.model.providerID] = info.model;
      await ctx.storage.set(seenKey, seen);
      await save();
      await recover();
    } else if (event.type === "session.execution.succeeded") {
      if (route) {
        route.pendingFailure = false;
        await save();
        await recover();
      }
    }
  };
  const registrations = [];
  registrations.push(
    await ctx.session.hook("retry", async (event: any) => {
      if (
        stopped ||
        event.sessionID !== sessionID ||
        event.error?.status !== 429 ||
        !isModel(event.model)
      )
        return;
      const selected = await owner(event.model);
      if (selected && (await quota(selected, true)) === "exhausted")
        event.decision = { retry: false };
    }),
  );
  registrations.push(
    await ctx.session.hook("prompt", async (event: any) => {
      if (stopped || event.sessionID !== sessionID) return;
      inputVersion++;
      await cancelManualChange();
      if (route) {
        route.pendingFailure = false;
        route.resume = false;
        await save();
      }
    }),
  );
  const guardBilling = async (event: any) => {
    if (stopped || !route || event.sessionID !== sessionID) return;
    if (!isModel(event.model))
      throw new Error(
        "Automatic subscription continuation requires a supported subscription provider",
      );
    const expected = [route.current, route.target, route.original].find(
      (selected) => selected?.model.providerID === event.model.providerID,
    );
    const connection = await ctx.integration.connection.active(event.model.providerID);
    const credential =
      connection?.type === "credential"
        ? await ctx.integration.connection.resolve(connection)
        : undefined;
    const headers = event.request?.headers;
    const hasApiKey =
      headers?.has("x-api-key") ||
      Object.keys(event.headers ?? {}).some((key) => key.toLowerCase() === "x-api-key");
    const hostname = new URL(event.request?.url ?? event.url).hostname;
    const expectedHost =
      event.model.providerID === "anthropic" ? "api.anthropic.com" : "chatgpt.com";
    const authorization =
      headers?.get("authorization") ??
      Object.entries(event.headers ?? {}).find(
        ([key]) => key.toLowerCase() === "authorization",
      )?.[1];
    if (
      !expected ||
      connection?.id !== expected.connection ||
      credential?.type !== "oauth" ||
      typeof credential.access !== "string" ||
      !credential.access ||
      authorization !== `Bearer ${credential.access}` ||
      (event.model.providerID === "openai" && accountOf(credential) !== expected.account) ||
      hasApiKey ||
      hostname !== expectedHost
    ) {
      throw new Error(
        "Automatic subscription continuation paused after an account, endpoint or API billing change",
      );
    }
    if (!(await billingEligible(expected))) {
      throw new Error("Automatic Claude continuation requires confirmed disabled extra usage");
    }
  };
  registrations.push(await ctx.session.hook("http.request", guardBilling));
  registrations.push(await ctx.session.hook("experimental.ws.handshake", guardBilling));
  const timer = setInterval(
    () =>
      recover().catch(() =>
        console.warn("cockpit plugin: subscription recovery paused; inspect its saved route"),
      ),
    30000,
  );
  return {
    handle,
    close: async () => {
      stopped = true;
      controller.abort();
      clearInterval(timer);
      for (const registration of registrations) await registration.dispose();
    },
  };
}
