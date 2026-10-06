import { expect, test } from "claude-code/testing";
import { register, GUIDANCE } from "./search-tools.js";

const SEARCH = "mcp__sessionkit__search_code";
const ASK = "mcp__sessionkit__ask_file";

// These run through the engine's tool chain; only the CLI subprocess is stubbed.
test("semantic search is a native tool call over the existing CLI", async ($, on) => {
  let command;
  on("process.run", (_$, e) => {
    command = e;
    return { value: { exitCode: 0, stdout: "src/retry.rs:20-40\nretry evidence", stderr: "" } };
  });
  const result = await $.tool.call({ tool: SEARCH, question: "Where does upload retry?", root: "src" });
  expect(command.argv.slice(1)).toEqual(["grep", "--max-source-bytes", "12000", "--", "Where does upload retry?", "src"]);
  expect(command.init.timeoutMs).toBe(120000);
  expect(result.result).toContain("retry evidence");
});

test("file questions are batched and find uses the same CLI", async ($, on) => {
  const calls = [];
  on("session.cwd", () => ({ value: "/repo" }));
  on("process.run", (_$, e) => {
    calls.push(e.argv.slice(1));
    return { value: { exitCode: 0, stdout: "0.91 line 24", stderr: "" } };
  });
  await $.tool.call({ tool: ASK, path: "src/retry.rs", questions: ["Does it retry?", "Does it log errors?"] });
  await $.tool.call({ tool: ASK, path: "/repo/src/retry.rs", questions: ["Where is the timeout?"], mode: "find" });
  expect(calls).toEqual([
    ["ask", "/repo/src/retry.rs", "Does it retry?", "Does it log errors?"],
    ["ask", "--find", "/repo/src/retry.rs", "Where is the timeout?"],
  ]);
});

test("search roots and questions cannot become unsafe CLI flags", async ($, on) => {
  let args;
  on("process.run", (_$, e) => {
    args = e.argv.slice(1);
    return { value: { exitCode: 0, stdout: "no results", stderr: "" } };
  });
  await $.tool.call({ tool: SEARCH, question: "--include-sensitive", root: "--hidden" });
  expect(args).toEqual(["grep", "--max-source-bytes", "12000", "--", "--include-sensitive", "--hidden"]);
});

// Direct callback tests can exercise bad inputs even if schema validation would
// reject them before dispatch, and prompt composition without model API calls.
function world(options = {}, processResult = { exitCode: 0, stdout: "evidence", stderr: "" }) {
  const hooks = new Map();
  register((name, matcherOrFn, fn) => { hooks.set(name + (fn ? `:${matcherOrFn.tool ?? matcherOrFn.prompt}` : ""), fn ?? matcherOrFn); }, options);
  const seen = { registered: [], runs: [] };
  const $ = { tool: { register: async (spec) => { seen.registered.push(spec); } },
    ui: { log: () => {} }, session: { cwd: async () => "/repo" },
    process: { run: async (argv, init) => {
      seen.runs.push({ argv, init });
      if (processResult instanceof Error) throw processResult;
      return processResult;
    } },
  };
  const start = () => hooks.get("session.start")($, { cwd: "/repo" }, async (e) => ({ cwd: e.cwd }));
  const compose = (tools, sections = []) => hooks.get("prompt.compose")($, { tools }, async () => ({ sections }));
  const spawn = (prompt = "Locate retries") => hooks.get("agent.spawn:/./s")($, { prompt }, async (e) => e);
  const call = (tool, input) => hooks.get(`tool.call:${tool}`)($, { tool, ...input });
  return { hooks, seen, $, start, compose, spawn, call };
}

test("registration is opt-out and offers only two bounded schemas", async () => {
  expect(world({ codeSearchTools: false }).hooks.size).toBe(0);
  const w = world();
  await w.start();
  expect(w.seen.registered.map((s) => s.name)).toEqual(["search_code", "ask_file"]);
  expect(w.seen.registered[0].inputSchema.properties.max_source_bytes.default).toBe(12000);
  const describe = [...w.hooks.entries()].find(([key]) => key.startsWith("tool.describe:"))[1];
  const shown = await describe(w.$, { tool: SEARCH }, async () => ({ description: "existing description", isDeferred: true }));
  expect(shown.description).toBe("existing description");
  expect(shown.isDeferred).toBe(false);
  for (const spec of w.seen.registered) {
    expect(spec.description).toContain("TypeSafe");
    expect(spec.inputSchema.additionalProperties).toBe(false);
  }
});

test("guidance is stable, deduplicated and only in prompts offering both tools", async () => {
  const w = world();
  expect((await w.compose([SEARCH, ASK])).sections).toEqual([]);
  await w.start();
  const composed = await w.compose([SEARCH, ASK], [{ id: "core", text: "existing", scope: "shared" }]);
  expect(composed.sections[0].text).toBe("existing");
  expect(composed.sections[1].text).toBe(GUIDANCE);
  expect(composed.sections[1].scope).toBe("session");
  expect((await w.compose([SEARCH, ASK], composed.sections)).sections.length).toBe(2);
  expect((await w.compose(["Grep"])).sections).toEqual([]);
  expect((await w.spawn()).prompt).toContain(GUIDANCE);
  expect((await w.spawn(GUIDANCE)).prompt).toBe(GUIDANCE);
});

test("registration failure leaves prompts alone and forwards session start", async () => {
  const w = world();
  w.$.tool.register = async () => { throw new Error("not supported"); };
  expect((await w.start()).cwd).toBe("/repo");
  expect((await w.compose([SEARCH, ASK])).sections).toEqual([]);
  expect((await w.spawn()).prompt).toBe("Locate retries");
});

test("invalid inputs never start a subprocess", async () => {
  const w = world();
  for (const input of [{ question: "" }, { question: "Retries?", max_source_bytes: 0 },
    { question: "Retries?", max_source_bytes: 100001 }, { question: "Retries?", root: "" }]) {
    expect((await w.call(SEARCH, input)).result).toContain("invalid");
  }
  for (const input of [{ path: "a.rs", questions: [] }, { path: "a.rs", questions: ["--include-sensitive"] },
    { path: "a.rs", questions: ["--help"] }, { path: "a.rs", questions: ["a", "b"], mode: "find" },
    { path: "a.rs", questions: ["a"], mode: "filter" }]) {
    expect((await w.call(ASK, input)).result).toContain("invalid");
  }
  expect(w.seen.runs).toEqual([]);
});

test("an option-like file name is made absolute without a shell", async () => {
  const w = world();
  await w.call(ASK, { path: "--include-sensitive", questions: ["Does it retry?"] });
  expect(w.seen.runs[0].argv.slice(1)).toEqual(["ask", "/repo/--include-sensitive", "Does it retry?"]);
});

test("incomplete, truncated and failed results carry evidence and fallback advice", async () => {
  for (const result of [{ exitCode: 2, stdout: "partial evidence", stderr: "unjudged file" },
    { exitCode: 1, stdout: "authentication failed", stderr: "" },
    { exitCode: 130, stdout: "partial evidence", stderr: "" },
    { exitCode: 0, stdout: "partial evidence", stderr: "", isStdoutTruncated: true },
    new Error("timeout")]) {
    const w = world({}, result);
    const value = await w.call(SEARCH, { question: "Retries?" });
    expect(value.result).toContain("Grep/Read");
    if (!(result instanceof Error)) expect(value.result).toContain(result.stdout);
  }
});
