// The tip after a large read, against the engine; the Read tool is stood in for beneath.
import { expect, test } from "claude-code/testing";

function world(on, lines) {
  on("session.id", () => ({ value: "session-1" }));
  on("session.cwd", () => ({ value: "/repo" }));
  on("tool.call", { tool: "Read" }, (_$, e) => {
    const content = "x\n".repeat(lines);
    return { result: { type: "text", file: { filePath: e.file_path, content, numLines: lines, startLine: 1, totalLines: lines } } };
  });
}

const tips = (result) => (result.context ?? []).filter((text) => text.startsWith("sessionkit:"));

test("a large whole-file read gets the tip once", async ($, on) => {
  world(on, 1748);
  const first = await $.tool.call({ tool: "Read", file_path: "/repo/src/start.rs" });
  expect(tips(first).length).toBe(1);
  expect(tips(first)[0]).toContain("sessionkit ask src/start.rs");
  const second = await $.tool.call({ tool: "Read", file_path: "/repo/src/next.rs" });
  expect(tips(second)).toEqual([]);
});

test("a small file gets no tip", async ($, on) => {
  world(on, 120);
  expect(tips(await $.tool.call({ tool: "Read", file_path: "/repo/a.rs" }))).toEqual([]);
});

test("a read of a range gets no tip", async ($, on) => {
  world(on, 1748);
  expect(tips(await $.tool.call({ tool: "Read", file_path: "/repo/a.rs", offset: 300, limit: 50 }))).toEqual([]);
});

test("a subagent gets its own tip", async ($, on) => {
  world(on, 1748);
  await $.tool.call({ tool: "Read", file_path: "/repo/a.rs" });
  const sub = await $.tool.call({ tool: "Read", file_path: "/repo/a.rs", agentId: "agent-1" });
  expect(tips(sub).length).toBe(1);
});

function running(on) {
  const runs = [];
  on("process.run", (_$, e) => {
    runs.push(e.argv.slice(1).join(" "));
    return { value: { exitCode: 0, stdout: JSON.stringify({ head: 20, declarations: ["120: pub fn start() {", "300: ## Install"] }), stderr: "" } };
  });
  return runs;
}

test("without the option a large read is whole", async ($, on) => {
  world(on, 1748);
  const runs = running(on);
  const result = await $.tool.call({ tool: "Read", file_path: "/repo/src/start.rs" });
  expect(result.result.file.numLines).toBe(1748);
  expect(runs).toEqual([]);
});

test("with the option the first whole read is an outline, the second is whole", { options: { readOutline: true } }, async ($, on) => {
  world(on, 1748);
  const runs = running(on);
  const first = await $.tool.call({ tool: "Read", file_path: "/repo/src/start.rs" });
  expect(first.result.file.numLines).toBe(20);
  expect(first.result.file.totalLines).toBe(1748);
  const note = first.context.find((text) => text.startsWith("sessionkit: src/start.rs has 1748 lines"));
  expect(note).toContain("120: pub fn start() {\n300: ## Install");
  expect(runs).toEqual(["trim --declarations"]);
  const second = await $.tool.call({ tool: "Read", file_path: "/repo/src/start.rs" });
  expect(second.result.file.numLines).toBe(1748);
});
