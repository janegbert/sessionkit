// sessionkit's architect: a repository onboarded with /sessionkit:architecture onboard has an
// architect, the agent type sessionkit:architect, with its memory in ARCHITECTURE.md. Each plan
// the coding agent presents and each Edit or Write it makes goes, after it is done and without
// waiting, to `sessionkit hook architect-check`, where Jev reads what the change does to the
// architecture in the memory. Ordinary work stops there. For a change Jev finds architectural,
// the loop that made it gets a note: the file, Jev's reading and the decision records it may go
// against. The coding agent checks a small case itself, or starts sessionkit:architect in the
// background for a view without its own reasoning, and puts a broken rule or a decision to the
// person. The plugin starts no agent: in auto mode the server-side classifier judges only calls a
// model request asked for, and refuses a spawn from a hook.
//
// An architect the model starts is held to ARCHITECTURE.md and docs/adr/ by the tool.call hook
// below, and its own changes are not checked.
//
// In a git repository without an onboarded memory, the first answer of a session is followed by
// one question: onboard an architect now, not now (asked again next session), or never for this
// repository, which `sessionkit hook architect-decline` keeps in ~/.config/sessionkit/architect.json.
//
// The option architect sets advise (the default), shadow (Jev's reading is logged in
// ~/.cache/sessionkit/architect-checks.jsonl, no note) or off. Without the memory, or without a
// TypeSafe key at hand, nothing is asked.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 30_000;
const ARCHITECT = "sessionkit:architect";
const PREFIX = "sessionkit architect";

const ONBOARD = "Onboard now";
const NOT_NOW = "Not now";
const NEVER = "Never for this project";

const architects = new Set(); // architects the model started
const noted = new Set(); // file and reading already noted this session, so a note is not repeated
let offered = false;

const reason = (error) => (error instanceof Error ? error.message : String(error));

/** The change a tool call made, as the classifier reads it, or null for one that changed nothing. */
function changeOf(tool, event, result) {
  const done = result.result;
  if (!done || done.staged) return null;
  if (tool === "ExitPlanMode") return done.plan ? { kind: "plan", file: "", change: done.plan } : null;
  const patch = done.gitDiff?.patch ?? (done.structuredPatch ?? []).flatMap((hunk) => hunk.lines).join("\n");
  if (tool === "Write" && done.type === "create") return { kind: "create", file: event.file_path, change: done.content };
  return patch ? { kind: "edit", file: event.file_path, change: patch } : null;
}

/** May an architect write this file? Only its memory. */
const allowed = (path) => /(^|\/)ARCHITECTURE\.md$/.test(path) || /(^|\/)docs\/adr\/[^/]+$/.test(path);

async function classify($, change, loop, options) {
  try {
    const input = { session_id: await $.session.id(), cwd: await $.session.cwd(), ...change };
    const run = await $.process.run([SESSIONKIT, "hook", "architect-check"], { stdin: JSON.stringify(input), timeoutMs: TIMEOUT_MS });
    const check = JSON.parse(run.stdout);
    if (!check.wake || options?.architect === "shadow") return;
    await notify($, loop, check);
  } catch (error) {
    $.ui.log(`${PREFIX}: change not checked (${reason(error)})`);
  }
}

/**
 * Jev's reading, as a note to the loop that made the change: what to check, and how, with the text
 * of each decision record it may go against, so the agent need not read them first. Once per
 * file and reading in a session. A subagent that has ended takes no rows; the main conversation does.
 */
export async function notify($, loop, check) {
  const against = check.against?.length ? `; it may go against ${check.against.join(", ")}` : "";
  const key = `${check.file}|${check.category}|${check.against ?? []}`;
  if (noted.has(key)) return;
  noted.add(key);
  const text = `${PREFIX} (Jev): ${check.file ? `the change to ${check.file}` : "the plan"} may affect the architecture in ` +
    `ARCHITECTURE.md: read as ${check.category} (${Number(check.relevance).toFixed(2)})${against}. Check it before you go on. ` +
    "For one file against one named decision record, hold it against that record and the matching part of ARCHITECTURE.md yourself. " +
    `Otherwise start the agent ${ARCHITECT} in the background with a prompt that starts with \`Review. Repository: ${check.root}.\` ` +
    "and gives the file and the change, not why you made it, so it judges the change on its own. If the change breaks a rule " +
    `or makes an architectural decision, put it to the person; a decision they take, ${ARCHITECT} records with a prompt ` +
    "that starts with `Record.`" +
    Object.entries(check.against_text ?? {}).map(([file, text]) => `\n\n${file}:\n${text}`).join("");
  const message = { type: "user", content: [{ type: "text", text }] };
  await $.session.append({ message, ...(loop ? { agentId: loop } : {}) })
    .catch(() => (loop ? $.session.append({ message }) : undefined))
    .catch((error) => $.ui.log(`${PREFIX}: Jev's reading not delivered (${reason(error)})`));
}

/** Offer an architect to a repository that has none and did not decline one; once per session. */
export async function offer($) {
  try {
    const run = await $.process.run([SESSIONKIT, "hook", "architect-offer"], { stdin: JSON.stringify({ cwd: await $.session.cwd() }), timeoutMs: TIMEOUT_MS });
    const check = JSON.parse(run.stdout);
    if (!check.ask) return;
    let choice;
    try {
      choice = await $.ui.ask(check.question, { options: [ONBOARD, NOT_NOW, NEVER], header: "Architect" });
    } catch {
      return; // dismissed, or nobody to ask: asked again next session
    }
    if (choice === ONBOARD) await $.prompt.submit({ text: "/sessionkit:architecture onboard" });
    if (choice === NEVER) {
      await $.process.run([SESSIONKIT, "hook", "architect-decline"], { stdin: JSON.stringify({ root: check.root }), timeoutMs: TIMEOUT_MS });
      $.ui.toast("sessionkit: no architect for this project. /sessionkit:architecture onboard still works when you change your mind.");
    }
  } catch (error) {
    $.ui.log(`${PREFIX}: no offer (${reason(error)})`);
  }
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (options?.architect === "off") return;

  on("agent.spawn", { subagentType: ARCHITECT }, async ($, event, next) => {
    const result = await next(event);
    if (result.agentId) architects.add(result.agentId);
    return result;
  });

  for (const tool of ["Edit", "Write", "ExitPlanMode"]) {
    on("tool.call", { tool }, async ($, event, next) => {
      const own = architects.has(event.agentId);
      if (own && tool !== "ExitPlanMode" && !allowed(event.file_path ?? "")) {
        return { deny: "The architect writes only ARCHITECTURE.md and docs/adr/." };
      }
      const result = await next(event);
      if (own || "deny" in result || result.isError) return result;
      const change = changeOf(tool, event, result);
      if (change) $.clock.after(0, () => classify($, change, event.agentId, options));
      return result;
    });
  }

  on("turn.complete", async ($, event, next) => {
    const result = await next(event);
    if (!event.agentId && event.reason === "answer" && !offered) {
      offered = true;
      $.clock.after(0, () => offer($));
    }
    return result;
  });
};
