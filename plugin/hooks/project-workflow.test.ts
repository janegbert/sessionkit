import { expect, test } from "claude-code/testing";
import { register, GUIDANCE } from "./project-workflow.js";

function world(options = {}) {
  const hooks = new Map();
  register((name, matcherOrFn, fn) => hooks.set(name, fn ?? matcherOrFn), options);
  // Any accidental file, process, network, model, UI or memory access fails.
  const $ = new Proxy({}, { get: (_target, key) => { throw new Error(`unexpected API access: ${String(key)}`); } });
  const compose = (sections = []) => hooks.get("prompt.compose")($, { tools: [] }, async () => ({ sections, untouched: "metadata" }));
  const spawn = (prompt = "Inspect this project's script configuration.") => hooks.get("agent.spawn")($,
    { prompt, description: "inspect", agentType: "Explore" }, async (event) => ({ ...event, agentId: "child" }));
  return { hooks, compose, spawn };
}

test("project workflow is strictly opt-in and registers only instruction hooks", () => {
  for (const options of [{}, { projectWorkflow: false }, { projectWorkflow: "true" }]) {
    expect(world(options).hooks.size).toBe(0);
  }
  expect([...world({ projectWorkflow: true }).hooks.keys()]).toEqual(["prompt.compose", "agent.spawn"]);
});

test("coaching is stable, deduplicated and preserves existing prompt sections", async () => {
  const w = world({ projectWorkflow: true });
  const existing = { id: "core", scope: "shared", text: "Repository instructions" };
  const result = await w.compose([existing]);
  expect(result.untouched).toBe("metadata");
  expect(result.sections[0]).toEqual(existing);
  expect(result.sections[1]).toEqual({ id: "sessionkit-project-workflow", scope: "session", text: GUIDANCE });
  expect((await w.compose(result.sections)).sections).toEqual(result.sections);
  expect((await w.compose()).sections[0].text).toBe(GUIDANCE);
});

test("subagents receive approval-bound guidance without changing spawn metadata", async () => {
  const w = world({ projectWorkflow: true });
  const result = await w.spawn();
  expect(result.prompt).toBe(`Inspect this project's script configuration.\n\n${GUIDANCE}`);
  expect(result.description).toBe("inspect");
  expect(result.agentType).toBe("Explore");
  expect(result.agentId).toBe("child");
  expect((await w.spawn(result.prompt)).prompt).toBe(result.prompt);
});

test("retention policy stays compact and makes non-persistence the default", () => {
  expect(GUIDANCE.length).toBeLessThan(2200);
  for (const rule of ["within the current task", "authoritative repository docs", "explicit user approval",
    "Subagents only return proposals", "including memory", "Do not create an entrypoint automatically",
    "<=60-line", "<=3 owned references", "<=2 focused helpers", "not enforced limits"]) {
    expect(GUIDANCE).toContain(rule);
  }
});

test("compose and spawn propagate downstream failures without swallowing them", async () => {
  const hooks = world({ projectWorkflow: true }).hooks;
  const next = async () => { throw new Error("downstream failure"); };
  for (const [name, event] of [["prompt.compose", {}], ["agent.spawn", { prompt: "Inspect" }]]) {
    let error;
    try { await hooks.get(name)({}, event, next); } catch (caught) { error = caught; }
    expect(error.message).toBe("downstream failure");
  }
});
