import { expect, test } from "claude-code/testing";
import { register } from "./permission-probe.js";

const input = { command: "printf sessionkit-probe" };

test("diagnostics never override a verdict or ask the user", async () => {
  let handler;
  register((_event, fn) => { handler = fn; }, { permissionProbe: true });
  for (const decision of ["allow", "ask", "deny"]) {
    const verdict = { decision, reason: "test reason" };
    for (const fails of [false, true]) {
      const $ = { ui: {
        log: () => { if (fails) throw new Error("log unavailable"); },
        ask: () => { throw new Error("must never ask"); },
      } };
      const result = await handler($, { tool: "Bash", input, tool_use_id: "real-call" }, async () => verdict);
      expect(result).toBe(verdict);
    }
  }
});

test("permission probe is off by default", async ($, on) => {
  on("tool.check", () => ({ decision: "deny", reason: "test denial" }));
  on("ui.log", () => { throw new Error("must not log"); });
  const result = await $.tool.check({ tool: "Bash", input });
  expect(result.decision).toBe("deny");
});

test("probe observes ask without treating it as classifier rejection", { options: { permissionProbe: true } }, async ($, on) => {
  on("tool.check", () => ({ decision: "ask", reason: "mode decider" }));
  const logs = [];
  on("ui.log", (_$, e) => { logs.push(e); });
  const result = await $.tool.check({ tool: "Bash", input });
  expect(result.decision).toBe("ask");
  expect(logs.length).toBe(1);
});
