// The effort route of a prompt, against the engine: the turn's model requests and sessionkit are
// stood in for beneath the plugin. Run with: claude plugin test plugin
import { expect, test } from "claude-code/testing";

/** Stand-ins for the world beneath the plugin; `answer` is what sessionkit prints, or an error it fails with. */
function world(on, answer) {
  const seen = { runs: [], inputs: [], timeouts: [], efforts: [] };
  on("session.id", () => ({ value: "session-1" }));
  on("prompt.submit", (_$, e) => ({ text: e.text }));
  on("turn.start", (_$, e) => ({ turnId: e.turnId }));
  on("turn.step", async function* (_$, e) {
    seen.efforts.push(e.effort);
    return { turnId: e.turnId, index: e.index, answer: "", toolUses: [], stopReason: "end_turn", usage: null };
  });
  on("process.run", (_$, e) => {
    // prompt.js asks first whether the session is cold
    if (e.argv[2] === "cold-check") return { value: { exitCode: 0, stdout: JSON.stringify({ cold: false }), stderr: "" } };
    seen.runs.push(e.argv.slice(2).join(" "));
    seen.inputs.push(JSON.parse(e.init.stdin));
    seen.timeouts.push(e.init.timeoutMs);
    if (answer instanceof Error) throw answer;
    return { value: { exitCode: 0, stdout: JSON.stringify(answer), stderr: "" } };
  });
  on("ui.log", () => ({ value: undefined }));
  return seen;
}

const LOW = { ok: true, proposal: { effort: "low" } };
const APPLY = { options: { applyEffort: true } };

const typed = (text) => ({ text, wait: false, origin: { kind: "composer" } });

/** A typed prompt whose turn starts. */
async function prompt($, text, turnId) {
  await $.prompt.submit(typed(text));
  await $.turn.start({ text, turnId });
}

/** One model request of a turn, read to its end. */
async function step($, input) {
  const stream = $.turn.step({ model: "claude-opus-5-5", messageCount: 3, ...input });
  for await (const _chunk of stream);
  await new Promise((resolve) => setTimeout(resolve, 0));
}

test("a typed prompt is routed once, beside the effort its turn runs at, and nothing changes", async ($, on) => {
  const seen = world(on, LOW);
  await prompt($, "ja", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "high" });
  await step($, { turnId: "turn-1", index: 1, effort: "high" });
  expect(seen.runs).toEqual(["effort-route"]);
  expect(seen.inputs[0]).toEqual({ session_id: "session-1", turn_id: "turn-1", prompt: "ja", model: "claude-opus-5-5", effort: "high" });
  expect(seen.efforts).toEqual(["high", "high"]);
});

test("a turn nobody typed is not routed", async ($, on) => {
  const seen = world(on, LOW);
  await $.prompt.submit({ text: "done", wait: false, origin: { kind: "peer", from: "other" } });
  await $.turn.start({ text: "done", turnId: "turn-1" });
  await step($, { turnId: "turn-1", index: 0, effort: "high" });
  expect(seen.runs).toEqual([]);
});

test("a subagent's steps and a model without effort are not routed", async ($, on) => {
  const seen = world(on, LOW);
  await prompt($, "ja", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "high", agentId: "agent-1" });
  await step($, { turnId: "turn-1", index: 0 });
  expect(seen.runs).toEqual([]);
  expect(seen.efforts).toEqual(["high", undefined]);
});

test("with the option every step of the turn runs at the lower effort, a subagent's not", APPLY, async ($, on) => {
  const seen = world(on, LOW);
  await prompt($, "commit this", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "high" });
  await step($, { turnId: "turn-1", index: 1, effort: "high" });
  await step($, { turnId: "turn-1", index: 0, effort: "high", agentId: "agent-1" });
  expect(seen.runs).toEqual(["effort-route"]);
  expect(seen.efforts).toEqual(["low", "low", "high"]);
});

test("with the option the effort is never raised", APPLY, async ($, on) => {
  const seen = world(on, { ok: true, proposal: { effort: "xhigh" } });
  await prompt($, "redesign the cache", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "medium" });
  expect(seen.efforts).toEqual(["medium"]);
});

test("with the option a late or failed answer leaves the whole turn as it was", APPLY, async ($, on) => {
  const seen = world(on, new Error("timed out"));
  await prompt($, "ja", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "high" });
  await step($, { turnId: "turn-1", index: 1, effort: "high" });
  expect(seen.runs).toEqual(["effort-route"]);
  expect(seen.timeouts).toEqual([2000]);
  expect(seen.efforts).toEqual(["high", "high"]);
});

test("the next turn is asked again", APPLY, async ($, on) => {
  const seen = world(on, LOW);
  await prompt($, "ja", "turn-1");
  await step($, { turnId: "turn-1", index: 0, effort: "high" });
  await prompt($, "and push", "turn-2");
  await step($, { turnId: "turn-2", index: 0, effort: "high" });
  expect(seen.inputs.map((input) => input.prompt)).toEqual(["ja", "and push"]);
  expect(seen.efforts).toEqual(["low", "low"]);
});
