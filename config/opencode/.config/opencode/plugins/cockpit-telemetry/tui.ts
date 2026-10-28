import { spawn } from "node:child_process";

const MAX_COUNT = 10_000;
const QUERY_TIMEOUT_MS = 2000;

export default {
  id: "cockpit-native-telemetry",
  setup: async (context: any) => {
    const sessionID = process.env.CLAUDE_CODE_SESSION_ID;
    if (!sessionID || !/^ses_[0-9A-Za-z]{12,40}$/.test(sessionID)) return;
    let stopped = false;
    let sampling = false;
    let sequence = 0;
    const cancellation = new AbortController();
    const publish = (counts: Record<string, number> | null) =>
      new Promise<void>((resolve, reject) => {
        sequence = Math.max(sequence + 1, Number(process.hrtime.bigint() / 1_000_000n));
        const child = spawn("cockpit-team", ["native-telemetry"], {
          stdio: ["pipe", "pipe", "pipe"],
        });
        const deadline = setTimeout(() => {
          child.kill();
          reject(new Error("native telemetry IPC timed out"));
        }, QUERY_TIMEOUT_MS);
        child.stdout.resume();
        child.stderr.resume();
        child.on("error", (error) => {
          clearTimeout(deadline);
          reject(error);
        });
        child.on("close", (code) => {
          clearTimeout(deadline);
          code === 0 ? resolve() : reject(new Error("native telemetry IPC refused"));
        });
        child.stdin.on("error", (error) => {
          clearTimeout(deadline);
          child.kill();
          reject(error);
        });
        child.stdin.end(JSON.stringify({ action: "native_telemetry", sequence, counts }));
      });
    const pendingCount = (items: unknown) => {
      if (
        !Array.isArray(items) ||
        items.length > MAX_COUNT ||
        items.some((item) => !item || item.sessionID !== sessionID || typeof item.id !== "string")
      ) {
        throw new Error("invalid native pending items");
      }
      return items.length;
    };
    const requireVisibleSession = () => {
      const route = context.ui.router.current();
      if (route?.type !== "session" || route.sessionID !== sessionID) {
        throw new Error("native visible session differs from cockpit session");
      }
    };
    const sample = async () => {
      if (stopped || sampling) return;
      sampling = true;
      let counts: Record<string, number> | null = null;
      try {
        requireVisibleSession();
        const options = {
          signal: AbortSignal.any([cancellation.signal, AbortSignal.timeout(QUERY_TIMEOUT_MS)]),
        };
        const [active, inbox, permissions, forms] = await Promise.all([
          context.client.session.active(options),
          context.client.session.inbox.list({ sessionID }, options),
          context.client.permission.list({ sessionID }, options),
          context.client.session.form.list({ sessionID }, options),
        ]);
        // Navigation during the native queries must not publish availability for the old view.
        requireVisibleSession();
        if (
          !active ||
          typeof active !== "object" ||
          Array.isArray(active) ||
          Object.keys(active).length > MAX_COUNT ||
          Object.entries(active).some(
            ([id, state]: [string, any]) =>
              !/^ses_[0-9A-Za-z]{12,40}$/.test(id) || state?.type !== "running",
          )
        ) {
          throw new Error("invalid native activity");
        }
        counts = {
          running_sessions: Object.keys(active).length,
          pending_inbox: pendingCount(inbox),
          pending_permissions: pendingCount(permissions),
          pending_forms: pendingCount(forms),
        };
      } catch {
        // A failed/malformed query invalidates availability; it must never turn into empty counts.
        counts = null;
      }
      try {
        if (!stopped) await publish(counts);
      } catch {
        console.error("cockpit plugin: native telemetry unavailable");
      } finally {
        sampling = false;
      }
    };
    await sample();
    const interval = Math.max(
      250,
      Math.min(10_000, Number(process.env.COCKPIT_TEAM_POLL_MS) || 3000),
    );
    const timer = setInterval(() => {
      void sample();
    }, interval);
    return () => {
      stopped = true;
      clearInterval(timer);
      cancellation.abort();
    };
  },
};
