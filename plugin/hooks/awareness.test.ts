// Cost awareness: when a subagent hears what it costs, and what the main session hears when one
// ends. sessionkit's wording is stood in for. Run with: claude plugin test plugin
import { expect, test } from "claude-code/testing";
import { ended, reset, spawned, stepped } from "./awareness.js";

/** An engine with sessionkit's answers and what the plugin appended. */
function engine(text = "LINE") {
  const seen = { asked: [], appended: [], logs: [] };
  const $ = {
    session: {
      id: async () => "session-1",
      append: async ({ message, agentId }) => void seen.appended.push({ text: message.content[0].text, agentId }),
    },
    process: { run: async (argv, { stdin }) => {
      seen.asked.push({ hook: argv[2], input: JSON.parse(stdin) });
      return { exitCode: 0, stdout: JSON.stringify({ text }), stderr: "" };
    } },
    ui: { log: (t) => void seen.logs.push(t) },
  };
  return { seen, $ };
}

const usage = (context) => ({ input_tokens: 10, cache_read_input_tokens: context - 10, cache_creation_input_tokens: 0, output_tokens: 50 });

test("a subagent hears its cost when its context passes the first mark and at each doubling, nothing in between", async () => {
  reset({ reason: "R", first_note_at: 50_000 });
  spawned("a1", "find the parser");
  const { seen, $ } = engine();
  for (const context of [20_000, 60_000, 90_000, 125_000, 200_000, 250_000]) await stepped($, "a1", "claude-sonnet-5-5", usage(context));
  expect(seen.asked.map((a) => a.input.context)).toEqual([60_000, 125_000, 250_000]);
  expect(seen.asked[0]).toEqual({ hook: "awareness-agent", input: { session_id: "session-1", kind: "grown", agent_id: "a1", context: 60_000, model: "claude-sonnet-5-5" } });
  expect(seen.appended).toEqual([{ text: "LINE", agentId: "a1" }, { text: "LINE", agentId: "a1" }, { text: "LINE", agentId: "a1" }]);
});

test("when a subagent ends the main session hears what it took, once", async () => {
  reset({ reason: "R", first_note_at: 50_000 });
  spawned("a2", "review the diff");
  const { seen, $ } = engine("sessionkit: the subagent ended: 12 calls");
  await ended($, "a2");
  await ended($, "a2");
  expect(seen.asked).toEqual([{ hook: "awareness-agent", input: { session_id: "session-1", kind: "ended", agent_id: "a2", description: "review the diff" } }]);
  expect(seen.appended).toEqual([{ text: "sessionkit: the subagent ended: 12 calls", agentId: undefined }]);
});

test("an agent this session did not spawn, or a session without the option, gets nothing", async () => {
  reset(null);
  spawned("a3", "x");
  const { seen, $ } = engine();
  await stepped($, "a3", "claude-sonnet-5-5", usage(500_000));
  await ended($, "unknown");
  expect(seen.asked).toEqual([]);
  expect(seen.appended).toEqual([]);
});

test("an empty line from sessionkit appends nothing", async () => {
  reset({ reason: "R", first_note_at: 50_000 });
  spawned("a4", "x");
  const { seen, $ } = engine("");
  await ended($, "a4");
  expect(seen.appended).toEqual([]);
});
