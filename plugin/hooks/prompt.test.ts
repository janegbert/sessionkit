// The cold-cache dialog against the engine: sessionkit, the session and the person are stood in
// for beneath the plugin. Run with: claude plugin test plugin
import { expect, mock, test } from "claude-code/testing";

const COLD = {
  cold: true,
  cost: "$2.82",
  fresh: true,
  question: "The prompt cache of this session has expired. Where do you go on?",
  stop: "sessionkit: your message is back in the prompt box.",
};

/** Stand-ins for the world beneath the plugin; returns what each was asked. */
function world(on, check, answer) {
  const seen = { runs: [], inputs: [], asked: [], filled: [], entered: [] };
  on("session.id", () => ({ value: "session-1" }));
  on("process.run", (_$, e) => {
    seen.runs.push(e.argv.slice(2).join(" "));
    seen.inputs.push(JSON.parse(e.init.stdin));
    const command = e.argv.slice(1).join(" ");
    const out = e.argv[2] === "cold-check" ? check
      : command === "compact --hook" ? { plan: [{ keep: 0 }], summary: "kept 1/1 messages", counts: { cut: 1, dropped: 2 } }
      : command === "hook compacted" ? { found: true, before: 463000, after: 41000 }
      : { ok: true };
    return { value: { exitCode: 0, stdout: JSON.stringify(out), stderr: "" } };
  });
  on("tool.call", { tool: "AskUserQuestion" }, (_$, e) => {
    const question = e.questions[0];
    seen.asked.push(question.options.map((o) => o.label));
    if (answer === undefined) return { deny: "dismissed" };
    const label = question.options.map((o) => o.label).find((l) => l.startsWith(answer));
    return { result: { questions: e.questions, answers: { [question.question]: label } } };
  });
  on("prompt.fill", (_$, e) => {
    seen.filled.push(e.text);
    return { isFilled: true };
  });
  on("prompt.submit", (_$, e) => {
    seen.entered.push(e.text);
    return { text: e.text };
  });
  on("ui.log", (_$, e) => { seen.logs = [...(seen.logs ?? []), e.text]; return { value: undefined }; });
  on("ui.toast", () => ({ value: undefined }));
  on("session.compact", () => ({ messages: [] }));
  on("command.run", (_$, e) => {
    seen.commands = [...(seen.commands ?? []), `${e.command} ${e.args}`.trim()];
    return { text: "" };
  });
  return seen;
}

const LARGE = { cold: false, large: true, at: "375K", window: 375000, question: "This session has 463K tokens of context. Compact and continue?" };

const typed = (text) => ({ text, wait: false, origin: { kind: "composer" } });

test("a warm session goes through without a question", async ($, on) => {
  const seen = world(on, { cold: false });
  const result = await $.prompt.submit(typed("hello"));
  expect(result).toEqual({ text: "hello" });
  expect(seen.asked).toEqual([]);
});

test("continue here acks and lets the message through", async ($, on) => {
  const seen = world(on, COLD, "Continue here");
  const result = await $.prompt.submit(typed("hello"));
  expect(seen.asked).toEqual([["Compact and continue", "Continue here ($2.82)"]]);
  expect(seen.runs).toEqual(["cold-check", "cold-choice continue"]);
  expect(result).toEqual({ text: "hello" });
});

test("compact and continue hands the message over and keeps it out of this one", async ($, on) => {
  const seen = world(on, COLD, "Compact and continue");
  const result = await $.prompt.submit(typed("hello"));
  expect(seen.runs).toEqual(["cold-check", "cold-choice fresh"]);
  expect(seen.entered).toEqual([]);
  expect(result.drop).toContain("fresh one");
});

test("new work is offered clear and continue beside compact: /clear here, then the message, no fresh session", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, { ...COLD, route: "search" }, "Clear and continue");
  const result = await $.prompt.submit(typed("something new"));
  expect(seen.asked).toEqual([["Clear and continue", "Compact and continue", "Continue here ($2.82)"]]);
  expect(seen.runs).toEqual(["cold-check"]);
  expect(result.drop).toContain("clearing");
  await clock.advance(0);
  expect(seen.commands).toEqual(["clear"]);
  expect(seen.entered).toContain("something new");
});

test("without a way to open a fresh session, stop puts the message back", async ($, on) => {
  const seen = world(on, { ...COLD, fresh: false }, "Stop");
  const result = await $.prompt.submit(typed("hello"));
  expect(seen.asked).toEqual([["Compact and continue", "Continue here ($2.82)", "Stop"]]);
  expect(seen.filled).toEqual(["hello"]);
  expect(result).toEqual({ drop: COLD.stop });
});

test("without a handoff, compact and continue acks, runs /compact, and sends the message after it", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, { ...COLD, fresh: false }, "Compact and continue");
  const result = await $.prompt.submit(typed("go on"));
  expect(seen.runs).toEqual(["cold-check", "cold-choice continue"]);
  expect(result.drop).toContain("compacting first");
  await clock.settle();
  expect(seen.commands).toEqual(["compact"]);
  await $.session.compact({ trigger: "manual", messages: [{ role: "user", text: "hello", toolUses: [] }] });
  await clock.settle();
  expect(seen.entered).toContain("go on");
});

test("a dismissed dialog puts the message back", async ($, on) => {
  const seen = world(on, COLD, undefined);
  const result = await $.prompt.submit(typed("hello"));
  expect(seen.filled).toEqual(["hello"]);
  expect(seen.entered).toEqual([]);
  expect(result).toEqual({ drop: COLD.stop });
});

test("a message from a peer is not asked about", async ($, on) => {
  const seen = world(on, COLD, "Continue here");
  await $.prompt.submit({ text: "hello", wait: false, origin: { kind: "peer", from: "other" } });
  expect(seen.runs).toEqual([]);
});

test("a large session: compact and continue runs /compact after the hook, and the message goes out after it", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, LARGE, "Compact and continue");
  const result = await $.prompt.submit(typed("go on"));
  expect(seen.asked).toEqual([["Compact and continue", "Continue", "Always at 375K"]]);
  expect(result.drop).toContain("compacting first");
  await clock.settle();
  expect(seen.commands).toEqual(["compact"]);
  await $.session.compact({ trigger: "manual", messages: [{ role: "user", text: "hello", toolUses: [] }] });
  await clock.settle();
  expect(seen.entered).toContain("go on");
  expect((seen.logs ?? []).filter((line) => line.includes("did not start"))).toEqual([]);
});

test("always: Claude Code compacts by itself from now on, and this time too", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, LARGE, "Always at 375K");
  await $.prompt.submit(typed("go on"));
  await clock.settle();
  expect(seen.commands).toEqual(["autocompact 375000", "compact"]);
});

const SUBAGENT = { cold: false, subagent: true, at: "375K", window: 375000, question: "A subagent of this session has 410K tokens of context. Set the window?" };

test("a large subagent: the window is set without a /compact, and the message goes out after it", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, SUBAGENT, "Always at 375K");
  const result = await $.prompt.submit(typed("go on"));
  expect(seen.asked).toEqual([["Always at 375K", "Continue"]]);
  expect(result.drop).toContain("auto-compact window");
  await clock.settle();
  expect(seen.commands).toEqual(["autocompact 375000"]);
  expect(seen.entered).toEqual(["go on"]);
});

test("a large subagent: declining sends the message and sets nothing", async ($, on) => {
  const seen = world(on, SUBAGENT, "Continue");
  const result = await $.prompt.submit(typed("go on"));
  expect(result.text).toBe("go on");
  expect(seen.commands).toBeUndefined();
});

test("a large subagent: a dismissed dialog sends the message and sets nothing", async ($, on) => {
  const seen = world(on, SUBAGENT, undefined);
  const result = await $.prompt.submit(typed("go on"));
  expect(result.text).toBe("go on");
  expect(seen.commands).toBeUndefined();
});

test("a large session can go on as it is", async ($, on) => {
  const seen = world(on, LARGE, "Continue");
  const result = await $.prompt.submit(typed("go on"));
  expect(result.text).toBe("go on");
  expect(seen.commands).toBeUndefined();
});
