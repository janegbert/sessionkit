// The message after a /compact, against the engine: sessionkit and the session are stood in for
// beneath the plugin. Run with: claude plugin test plugin
import { expect, test } from "claude-code/testing";

const PLAN = { plan: [{ keep: 0 }], summary: "kept 1/1 messages, no summary", counts: { pinned: 0, kept: 0, cut: 12, dropped: 586 } };

function world(on, answer) {
  const seen = { runs: [], submitted: [], logs: [] };
  on("session.id", () => ({ value: "session-1" }));
  on("process.run", (_$, e) => {
    const command = e.argv.slice(1).join(" ");
    seen.runs.push(command);
    const out = command === "compact --hook" ? answer : { found: true, before: 912612, after: 39312 };
    return { value: { exitCode: 0, stdout: JSON.stringify(out), stderr: "" } };
  });
  on("ui.log", (_$, e) => { seen.logs.push(e.text); return { value: undefined }; });
  on("ui.toast", () => ({ value: undefined }));
  on("session.compact", () => ({ messages: [{ role: "user", text: "core summary", toolUses: [] }] })); // core, beneath the plugin
  on("prompt.submit", (_$, e) => {
    seen.submitted.push(e.text);
    return { text: e.text };
  });
  return seen;
}

const MESSAGES = [{ role: "user", text: "hello", toolUses: [] }];
const settle = () => new Promise((resolve) => setTimeout(resolve, 10));

test("after a /compact the agent is asked to report it with Claude Code's numbers", async ($, on) => {
  const seen = world(on, PLAN);
  await $.session.compact({ trigger: "manual", messages: MESSAGES });
  await settle();
  expect(seen.logs).toEqual(["sessionkit: kept 1/1 messages, no summary"]);
  expect(seen.runs).toEqual(["compact --hook", "hook compacted"]);
  expect(seen.submitted.length).toBe(1);
  expect(seen.submitted[0]).toContain("went from 913K to 39K tokens");
  expect(seen.submitted[0]).toContain("586 tool calls were dropped and 12 tool outputs were cut");
  expect(seen.submitted[0]).toContain("in the language of this conversation");
});

test("an auto-compaction asks nothing: the agent goes on with its work", async ($, on) => {
  const seen = world(on, PLAN);
  await $.session.compact({ trigger: "auto", messages: MESSAGES });
  await settle();
  expect(seen.runs).toEqual(["compact --hook"]);
  expect(seen.submitted).toEqual([]);
});

test("a fallback to the summary asks nothing", async ($, on) => {
  const seen = world(on, { fallback: "below the minimum" });
  await $.session.compact({ trigger: "manual", messages: MESSAGES });
  await settle();
  expect(seen.submitted).toEqual([]);
});
