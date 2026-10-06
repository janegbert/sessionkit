// The shadow route of a subagent, against the engine: the spawn and sessionkit are stood in for
// beneath the plugin. Run with: claude plugin test plugin
import { expect, test } from "claude-code/testing";

function world(on, spawned) {
  const seen = { runs: [], inputs: [] };
  on("session.id", () => ({ value: "session-1" }));
  on("agent.spawn", () => spawned);
  on("process.run", (_$, e) => {
    seen.runs.push(e.argv.slice(2).join(" "));
    seen.inputs.push(JSON.parse(e.init.stdin));
    return { value: { exitCode: 0, stdout: JSON.stringify({ ok: true }), stderr: "" } };
  });
  return seen;
}

const SPAWN = { parentModel: "claude-opus-5-5", tool_use_id: "toolu_1", prompt: "Find where the cache is read.", description: "Find cache reads", subagentType: "Explore" };

test("a spawn goes through unchanged and its route is logged", async ($, on) => {
  const seen = world(on, { model: "claude-sonnet-5-5", agentId: "agent-1" });
  const result = await $.agent.spawn(SPAWN);
  expect(result).toEqual({ model: "claude-sonnet-5-5", agentId: "agent-1" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(seen.runs).toEqual(["agent-route"]);
  expect(seen.inputs[0]).toEqual({
    session_id: "session-1", agent_id: "agent-1", agent_type: "Explore", description: "Find cache reads",
    prompt: "Find where the cache is read.", model: "claude-sonnet-5-5", requested: null, parent_model: "claude-opus-5-5",
  });
});

test("a refused spawn logs nothing", async ($, on) => {
  const seen = world(on, { deny: "not allowed" });
  const result = await $.agent.spawn(SPAWN);
  expect(result).toEqual({ deny: "not allowed" });
  expect(seen.runs).toEqual([]);
});
