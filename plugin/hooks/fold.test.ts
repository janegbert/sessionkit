// The fold of a finished turn, against the engine: sessionkit and the session are stood in for
// beneath the plugin. Run with: claude plugin test plugin
import { expect, test } from "claude-code/testing";
import { FOLD } from "./compact.js";

const LONG = "line\n".repeat(5000);
const CUT = "line\n[sessionkit compact cut 24700 chars of this tool result; re-run the tool if needed]";

/** Two turns; the second read a file of `output`. */
const conversation = (output) => [
  { role: "user", text: "read a.rs", toolUses: [], handle: "h0" },
  { role: "assistant", text: "read it", toolUses: [], handle: "h1" },
  { role: "user", text: "now b.rs", toolUses: [], handle: "h2" },
  { role: "assistant", text: "", toolUses: [{ tool_use_id: "u1", tool: "Read", input: { file_path: "b.rs" }, text: output }], handle: "h3" },
  { role: "user", text: "", toolUses: [], toolResults: [{ tool_use_id: "u1", text: output, isError: false }], handle: "h4" },
  { role: "assistant", text: "done", toolUses: [], handle: "h5" },
];

const PLAN = { plan: [{ keep: 0 }, { keep: 1 }, { keep: 2 }, { keep: 3 }, { from: 4, toolUses: [], toolResults: [{ keep: 0, text: CUT }] }, { keep: 5 }], summary: "folded the last turn: 1 tool output(s) cut, 24,700 characters" };
const TURN = { answer: "done", durationMs: 1200, isAborted: false, turnId: "turn-1", reason: "answer" };

function world(on, messages, answer) {
  const seen = { runs: [], compactions: [], logs: [] };
  on("session.messages", () => ({ value: messages }));
  on("process.run", (_$, e) => {
    seen.runs.push(e.argv.slice(1).join(" "));
    const out = e.argv[2] === "--fold" ? answer : { fallback: "no tool calls" };
    return { value: { exitCode: 0, stdout: JSON.stringify(out), stderr: "" } };
  });
  on("ui.log", (_$, e) => { seen.logs.push(e.text); return { value: undefined }; });
  on("ui.toast", () => ({ value: undefined }));
  on("turn.complete", (_$, e) => ({ text: e.answer }));
  // Core, beneath the plugin: a fold never gets here.
  on("session.compact", (_$, e) => {
    seen.compactions.push(e.instructions);
    return { messages: [{ role: "user", text: "core summary", toolUses: [] }] };
  });
  return seen;
}

test("with the option off a turn with a long tool output is left alone", async ($, on) => {
  const seen = world(on, conversation(LONG), PLAN);
  await $.turn.complete(TURN);
  expect(seen.runs).toEqual([]);
});

test("a turn with little tool output is not folded", { options: { foldTurns: true } }, async ($, on) => {
  const seen = world(on, conversation("fn main() {}"), PLAN);
  await $.turn.complete(TURN);
  expect(seen.runs).toEqual([]);
});

test("a subagent's turn is not folded", { options: { foldTurns: true } }, async ($, on) => {
  const seen = world(on, conversation(LONG), PLAN);
  await $.turn.complete({ ...TURN, agentId: "agent-1" });
  expect(seen.runs).toEqual([]);
});

// The test engine stamps no trigger and no messages on a compaction that a plugin starts, so here
// it passes the fold's hook by and lands at core. What this shows: the compaction was started,
// marked as a fold, and sessionkit compact left it alone. In a session it carries the trigger
// `plugin` and the fold's hook answers it; that path is tested live (see fold.js).
test("a turn with a long tool output starts a compaction marked as a fold", { options: { foldTurns: true } }, async ($, on) => {
  const seen = world(on, conversation(LONG), PLAN);
  await $.turn.complete(TURN);
  expect(seen.compactions).toEqual([FOLD]);
  expect(seen.runs).toEqual([]);
});

test("the fold keeps every message by its handle and rebuilds only the cut tool result", { options: { foldTurns: true } }, async ($, on) => {
  const messages = conversation(LONG);
  world(on, messages, PLAN);
  const result = await $.session.compact({ trigger: "plugin", instructions: FOLD, messages });
  expect(result.messages.map((m) => m.handle)).toEqual(["h0", "h1", "h2", "h3", undefined, "h5"]);
  expect(result.messages[3].toolUses[0].text).toBe(LONG);
  expect(result.messages[4]).toEqual({ role: "user", text: "", toolUses: [], toolResults: [{ tool_use_id: "u1", text: CUT, isError: false }] });
});

test("when sessionkit finds nothing to cut the conversation stays as it is", { options: { foldTurns: true } }, async ($, on) => {
  const messages = conversation(LONG);
  const seen = world(on, messages, { fallback: "no long tool output in the last turn" });
  const result = await $.session.compact({ trigger: "plugin", instructions: FOLD, messages });
  expect(result.skip).toContain("no long tool output in the last turn");
  expect(seen.compactions).toEqual([]);
});

test("another plugin's compaction is not a fold", { options: { foldTurns: true } }, async ($, on) => {
  const messages = conversation(LONG);
  const seen = world(on, messages, PLAN);
  await $.session.compact({ trigger: "plugin", instructions: "keep the plan", messages });
  expect(seen.runs).toEqual(["compact --hook"]);
  expect(seen.compactions).toEqual(["keep the plan"]);
});
