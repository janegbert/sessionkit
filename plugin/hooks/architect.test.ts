// The architect, against the engine: sessionkit (Jev's reading), the tools, the subagents and the
// person are stood in for beneath the plugin. Run with: claude plugin test plugin
//
// The note's way into a loop that has ended is tested through notify with the test's own engine;
// the whole loop was run live.
import { expect, mock, test } from "claude-code/testing";
import { notify, offer } from "./architect.js";

const QUIET = { wake: false, relevance: 0.02, category: "local" };
const LOUD = { wake: true, relevance: 0.97, category: "boundary", root: "/repo", file: "src/checkout/complete.rs",
  against: ["docs/adr/0002-billing-only-writes-invoices.md"],
  against_text: { "docs/adr/0002-billing-only-writes-invoices.md": "# Billing only writes invoices\n\nOther modules go through BillingGateway." } };

/** Stand-ins for the world beneath the plugin; `checks` answers each architect-check in turn. */
function world(on, checks) {
  const seen = { checked: [], spawns: [], appended: [], asked: [], logs: [] };
  const answers = [...checks];
  on("session.id", () => ({ value: "session-1" }));
  on("session.cwd", () => ({ value: "/repo" }));
  on("process.run", (_$, e) => {
    if (e.argv[2] !== "architect-check") return { value: { exitCode: 0, stdout: "{}", stderr: "" } };
    seen.checked.push(JSON.parse(e.init.stdin));
    return { value: { exitCode: 0, stdout: JSON.stringify(answers.shift() ?? QUIET), stderr: "" } };
  });
  on("tool.call", { tool: "Edit" }, (_$, e) => ({
    result: { filePath: e.file_path, oldString: e.old_string, newString: e.new_string, originalFile: "", userModified: false, replaceAll: false,
      structuredPatch: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: [`-${e.old_string}`, `+${e.new_string}`] }] },
  }));
  on("tool.call", { tool: "Write" }, (_$, e) => ({ result: { type: "create", filePath: e.file_path, content: e.content, structuredPatch: [], originalFile: null } }));
  on("tool.call", { tool: "ExitPlanMode" }, () => ({ result: { plan: "1. Checkout writes invoice rows itself.", isAgent: false } }));
  // A plugin's spawn reaches this in the Agent tool's words, a test's own in the event's.
  on("agent.spawn", (_$, e) => {
    seen.spawns.push({ type: e.subagentType ?? e.subagent_type, prompt: e.prompt });
    return { model: "claude-sonnet-5-5", agentId: `architect-${seen.spawns.length}` };
  });
  on("session.append", (_$, e, next) => {
    seen.appended.push({ text: e.message.content[0].text, agentId: e.agentId });
    return next(e);
  });
  on("ui.log", (_$, e) => { seen.logs.push(e.text); return { value: undefined }; });
  return seen;
}

const EDIT = { tool: "Edit", file_path: "/repo/src/checkout/complete.rs", old_string: "let t = 1;", new_string: "use crate::billing::persistence::InvoiceRepository;" };
/** The test's engine with the display a plugin's own has: its log, and the person's `choice`. */
const engine = ($, seen, choice) => ({
  ...$,
  ui: {
    log: (text) => void seen.logs.push(text),
    ask: async (question, { options }) => {
      seen.asked.push({ question, options });
      if (choice === undefined) throw new Error("dismissed");
      return choice;
    },
  },
});

const NOTE = "sessionkit architect (Jev): the change to src/checkout/complete.rs may affect the architecture in ARCHITECTURE.md: " +
  "read as boundary (0.97); it may go against docs/adr/0002-billing-only-writes-invoices.md. Check it before you go on. " +
  "For one file against one named decision record, hold it against that record and the matching part of ARCHITECTURE.md yourself. " +
  "Otherwise start the agent sessionkit:architect in the background with a prompt that starts with `Review. Repository: /repo.` " +
  "and gives the file and the change, not why you made it, so it judges the change on its own. If the change breaks a rule " +
  "or makes an architectural decision, put it to the person; a decision they take, sessionkit:architect records with a prompt " +
  "that starts with `Record.`\n\ndocs/adr/0002-billing-only-writes-invoices.md:\n# Billing only writes invoices\n\nOther modules go through BillingGateway.";

test("an ordinary edit is checked and wakes nothing", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [QUIET]);
  const result = await $.tool.call({ ...EDIT, new_string: "let total = 1;" });
  expect(result.result.filePath).toBe(EDIT.file_path);
  await clock.settle();
  expect(seen.checked).toEqual([{ session_id: "session-1", cwd: "/repo", kind: "edit", file: EDIT.file_path, change: "-let t = 1;\n+let total = 1;" }]);
  expect(seen.appended).toEqual([]);
});

test("an architectural edit is checked, and the plugin starts no agent", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [LOUD]);
  await $.tool.call(EDIT);
  await clock.settle();
  expect(seen.checked.length).toBe(1);
  expect(seen.logs).toEqual([]);
  expect(seen.spawns).toEqual([]);
});

test("Jev's reading goes to the loop as a note, once per file and reading", async ($, on) => {
  const seen = world(on, []);
  const reading = { ...LOUD, file: "src/checkout/refund.rs" };
  await notify(engine($, seen), undefined, reading);
  await notify(engine($, seen), undefined, reading);
  expect(seen.appended).toEqual([{ text: NOTE.replace("complete.rs", "refund.rs"), agentId: undefined }]);
});

test("a plan is checked as a plan", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [QUIET]);
  await $.tool.call({ tool: "ExitPlanMode" });
  await clock.settle();
  expect(seen.checked[0]).toMatchObject({ kind: "plan", file: "", change: "1. Checkout writes invoice rows itself." });
});

test("a new file is checked with its content", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [QUIET]);
  await $.tool.call({ tool: "Write", file_path: "/repo/src/checkout/invoice.rs", content: "pub struct Invoice;" });
  await clock.settle();
  expect(seen.checked[0]).toMatchObject({ kind: "create", file: "/repo/src/checkout/invoice.rs", change: "pub struct Invoice;" });
});

test("in shadow Jev's reading is logged by sessionkit and no note goes out", { options: { architect: "shadow" } }, async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [LOUD]);
  await $.tool.call(EDIT);
  await clock.settle();
  expect(seen.checked.length).toBe(1);
  expect(seen.appended).toEqual([]);
});

test("off checks nothing", { options: { architect: "off" } }, async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, [LOUD]);
  await $.tool.call(EDIT);
  await clock.settle();
  expect(seen.checked).toEqual([]);
});

test("an architect writes only its memory, and its writes are not checked", async ($, on) => {
  const clock = mock.clock(on);
  const seen = world(on, []);
  const { agentId } = await $.agent.spawn({ subagentType: "sessionkit:architect", prompt: "Onboard. Repository: /repo." });
  const code = await $.tool.call({ ...EDIT, agentId });
  expect(code.deny).toContain("The architect writes only ARCHITECTURE.md and docs/adr/");
  const memory = await $.tool.call({ tool: "Write", file_path: "/repo/ARCHITECTURE.md", content: "# Architecture", agentId });
  expect(memory.deny).toBeUndefined();
  const adr = await $.tool.call({ tool: "Write", file_path: "/repo/docs/adr/0005-billing-owns-invoices.md", content: "# Billing", agentId });
  expect(adr.deny).toBeUndefined();
  await clock.settle();
  expect(seen.checked).toEqual([]);
});

test("a note for a loop that has ended goes to the main conversation", async ($, on) => {
  const seen = world(on, []);
  await notify(engine($, seen), "worker-1", { ...LOUD, file: "", category: "structure", against: [], against_text: {} });
  const text = expect.stringContaining("the plan may affect the architecture in ARCHITECTURE.md: read as structure (0.97). Check it");
  // No loop worker-1 runs in the test, so the engine refuses the first row and the main conversation takes it.
  expect(seen.appended).toEqual([{ agentId: "worker-1", text }, { agentId: undefined, text }]);
});

/** The engine for offer: sessionkit's answers per hook, and what the plugin did with them. */
function offering(answer, choice) {
  const seen = { asked: [], ran: [], submitted: [], toasts: [], logs: [] };
  const $ = {
    session: { cwd: async () => "/repo/src" },
    process: { run: async (argv, { stdin }) => {
      seen.ran.push({ hook: argv[2], input: JSON.parse(stdin) });
      return { exitCode: 0, stdout: JSON.stringify(argv[2] === "architect-offer" ? answer : { saved: "x" }), stderr: "" };
    } },
    prompt: { submit: async ({ text }) => void seen.submitted.push(text) },
  };
  return { seen, $: { ...engine($, seen, choice), ui: { ...engine($, seen, choice).ui, toast: (text) => void seen.toasts.push(text) } } };
}

const OFFER = { ask: true, root: "/repo", question: "repo has no architect yet. Onboard one?" };

test("a repository without an architect is offered one, and onboarding starts on yes", async () => {
  const { seen, $ } = offering(OFFER, "Onboard now");
  await offer($);
  expect(seen.ran[0]).toEqual({ hook: "architect-offer", input: { cwd: "/repo/src" } });
  expect(seen.asked).toEqual([{ question: OFFER.question, options: ["Onboard now", "Not now", "Never for this project"] }]);
  expect(seen.submitted).toEqual(["/sessionkit:architecture onboard"]);
});

test("never for this project is saved for that repository", async () => {
  const { seen, $ } = offering(OFFER, "Never for this project");
  await offer($);
  expect(seen.ran[1]).toEqual({ hook: "architect-decline", input: { root: "/repo" } });
  expect(seen.submitted).toEqual([]);
  expect(seen.toasts.length).toBe(1);
});

test("not now, a dismissed question or a repository that needs no offer changes nothing", async () => {
  for (const [answer, choice] of [[OFFER, "Not now"], [OFFER, undefined], [{ ask: false, reason: "declined" }, "Onboard now"]]) {
    const { seen, $ } = offering(answer, choice);
    await offer($);
    expect(seen.ran.map((r) => r.hook)).toEqual(["architect-offer"]);
    expect(seen.submitted).toEqual([]);
  }
});
