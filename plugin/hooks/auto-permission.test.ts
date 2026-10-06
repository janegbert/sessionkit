import { expect, test } from "claude-code/testing";
import { register } from "./auto-permission.js";

const input = { command: "printf sessionkit-retry-probe" };

// The test engine exposes the classic event without manufacturing a classifier
// verdict. This checks registration/forwarding, not a live classifier rejection.
test("PermissionDenied remains unchanged without an active call", { options: { askOnAutoDenial: true } }, async ($, on) => {
  on("ui.log", () => { throw new Error("must not log denial arguments"); });
  on("classic.PermissionDenied", () => ({}));
  const result = await $.classic.PermissionDenied({ tool_name: "Bash", tool_input: input,
    tool_use_id: "no-active-call", reason: "[Test] denied" });
  expect(result.retry).toBeUndefined();
});

test("Auto approval is opt-in and registers its three hooks only when enabled", async () => {
  const disabled = [];
  register((name) => { disabled.push(name); }, {});
  expect(disabled).toEqual([]);
  const enabled = [];
  register((name) => { enabled.push(name); }, { askOnAutoDenial: true });
  expect(enabled).toEqual(["classic.PermissionDenied", "tool.check", "tool.call"]);
});

// Controlled continuation tests: no command executes. Simulate core emitting
// PermissionDenied after ask, then returning the permission error to tool.call.
function world(config = {}) {
  const handlers = {};
  register((name, matcherOrFn, fn) => { handlers[name] = fn ?? matcherOrFn; }, { askOnAutoDenial: true });
  const signal = { aborted: false };
  const seen = { attempts: 0, executions: 0, questions: 0, ids: [], question: null };
  const $ = { ui: { log: () => {}, ask: async (question, choices) => {
    seen.questions += 1;
    seen.question = { question, choices };
    if (config.duringAsk) await config.duringAsk(check);
    if (config.cancel) signal.aborted = true;
    if (config.answer instanceof Error) throw config.answer;
    return config.answer ?? "Allow once";
  } } };
  const continuation = (fn) => Object.assign(fn, { signal });
  async function check(id, args = input, verdict = { decision: "ask" }, extra = {}) {
    return handlers["tool.check"]($, { tool: "Bash", input: args, tool_use_id: id, ...extra },
      continuation(async () => verdict));
  }
  async function call(id = "call-1", extra = {}) {
    let attempt = 0;
    return handlers["tool.call"]($, { tool: "Bash", ...input, tool_use_id: id, ...extra },
      continuation(async (event) => {
        seen.attempts += 1;
        seen.ids.push(event.tool_use_id);
        attempt += 1;
        const args = attempt === 2 && config.changedInput ? config.changedInput : input;
        const verdict = attempt === 2 && config.retryVerdict ? config.retryVerdict : { decision: "ask" };
        const checked = await check(id, args, verdict, extra);
        if (config.throwOnAttempt === attempt) throw new Error("continuation failed");
        if (checked.decision === "allow" || config.success) {
          seen.executions += 1;
          return { result: "printed", text: "printed" };
        }
        if (config.emitDenial !== false) await handlers["classic.PermissionDenied"]($, {
          tool_name: "Bash", tool_input: config.denialInput ?? args, tool_use_id: id,
          reason: config.reason ?? "[Test classifier rule] denied", agent_id: extra.agentId,
        }, continuation(async () => ({})));
        if (config.successAfterDenial) return { result: "printed", text: "printed" };
        return { result: "permission denied", text: "permission denied", isError: true };
      }));
  }
  return { call, check, seen };
}

test("approval retries the same id once and cannot leak after the call", async () => {
  const { call, check, seen } = world();
  expect((await call()).isError).toBeUndefined();
  expect(seen.attempts).toBe(2);
  expect(seen.executions).toBe(1);
  expect(seen.ids).toEqual(["call-1", "call-1"]);
  expect(seen.question.question).toContain(input.command);
  expect(seen.question.question).toContain("[Test classifier rule] denied");
  expect(seen.question.question).toContain("does not save a permission rule");
  expect(seen.question.choices).toEqual(["Keep denied", "Allow once"]);
  expect((await check("call-1")).decision).toBe("ask");
});

test("decline, free text, dismissal and interruption do not retry", async () => {
  for (const config of [{ answer: "Keep denied" }, { answer: "yes" },
    { answer: new Error("dismissed") }, { cancel: true }]) {
    const { call, seen } = world(config);
    expect((await call()).isError).toBe(true);
    expect(seen.attempts).toBe(1);
    expect(seen.executions).toBe(0);
  }
});

test("error text alone and no-verdict denials never trigger approval", async () => {
  for (const config of [{ emitDenial: false }, { reason: "Classifier unavailable" },
    { reason: "Auto mode could not evaluate this action and is blocking it for safety: test" },
    { reason: "" }, { denialInput: { command: "another command" } }]) {
    const { call, seen } = world(config);
    expect((await call()).isError).toBe(true);
    expect(seen.questions).toBe(0);
    expect(seen.attempts).toBe(1);
  }
});

test("changed arguments, explicit rules, hook decisions and ceilings retain normal checks", async () => {
  for (const config of [{ changedInput: { command: "another command" } },
    { retryVerdict: { decision: "deny", rule: "Bash(*)" } },
    { retryVerdict: { decision: "ask", hook: "PreToolUse" } },
    { retryVerdict: { decision: "ask", ceiling: "ask" } }]) {
    const { call, seen } = world(config);
    expect((await call()).isError).toBe(true);
    expect(seen.executions).toBe(0);
    expect(seen.attempts).toBe(2); // no third attempt after the second denial
    expect(seen.questions).toBe(1);
  }
});

test("another call cannot consume this call's approval", async () => {
  const { call, seen } = world({ duringAsk: async (check) => {
    expect((await check("other-call")).decision).toBe("ask");
  } });
  await call();
  expect(seen.executions).toBe(1);
});

test("continuation failure removes pending approval", async () => {
  for (const attempt of [1, 2]) {
    const { call, check } = world({ throwOnAttempt: attempt });
    let failed = false;
    try { await call(); } catch { failed = true; }
    expect(failed).toBe(true);
    expect((await check("call-1")).decision).toBe("ask");
  }
});

test("successful calls and subagents are never replayed", async () => {
  for (const config of [{ success: true }, { successAfterDenial: true }]) {
    const successful = world(config);
    await successful.call();
    expect(successful.seen.attempts).toBe(1);
    expect(successful.seen.questions).toBe(0);
  }
  const subagent = world();
  await subagent.call("sub-call", { agentId: "agent-1" });
  expect(subagent.seen.attempts).toBe(1);
  expect(subagent.seen.questions).toBe(0);
});
