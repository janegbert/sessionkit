// sessionkit's choice before a message to a cold session: when the prompt cache has expired and
// writing the context again is expensive, ask in Claude Code's own dialog whether to go on here
// or compacted first, so the cache write holds only what matters. In a warm session whose context has grown large
// (375K tokens by default), ask whether to compact first, since every message re-reads it all. When
// a subagent of the session has grown that large and no auto-compact window is set, ask whether to
// set one, since without it a subagent never compacts. The dialog is the engine's, so asking costs no
// model call. `sessionkit hook cold-check` decides whether to ask and words the question;
// `sessionkit hook cold-choice` acts on the answer. When anything fails, the message goes through,
// and the settings hook `sessionkit hook prompt` still stops it once.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

import { waiting } from "./waiting.js";

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 10_000;

const STOP = "Stop";
const COMPACT = "Compact and continue";
const CLEAR = "Clear and continue";
const CONTINUE = "Continue";

async function sessionkit($, args, input) {
  const run = await $.process.run([SESSIONKIT, "hook", ...args], { stdin: JSON.stringify(input), timeoutMs: TIMEOUT_MS });
  return JSON.parse(run.stdout);
}

/**
 * A warm session with a large context: compact first, then send the message; or go on as it is;
 * or let Claude Code compact by itself from now on, at about 375K (its /autocompact), which also
 * fits a session in auto mode that runs on without new messages. A command cannot run from this
 * hook, which holds the turn, so it runs just after it; compact.js sends the message after /compact.
 */
async function large($, event, next, check) {
  const ALWAYS = `Always at ${check.at}`;
  let choice;
  try {
    choice = await $.ui.ask(check.question, { options: [COMPACT, CONTINUE, ALWAYS], header: "Large context" });
  } catch {
    choice = CONTINUE; // dismissed
  }
  if (choice === CONTINUE) return next(event);
  return compactFirst($, event, choice === ALWAYS ? [["autocompact", String(check.window)], ["compact", ""]] : [["compact", ""]]);
}

/** Run the commands just after this hook, which holds the turn; compact.js sends the message after /compact. */
function compactFirst($, event, commands) {
  waiting.text = event.text;
  $.clock.after(0, async () => {
    try {
      for (const [command, args] of commands) await $.command.run({ command, args });
    } catch (error) {
      waiting.text = null;
      $.ui.log(`sessionkit: /${commands.at(-1)[0]} did not start (${error instanceof Error ? error.message : String(error)})`);
      await $.prompt.fill({ text: event.text });
      $.ui.toast("sessionkit: could not start /compact; your message is back in the prompt box.");
    }
  });
  return { drop: "sessionkit: compacting first; your message goes out once that is done." };
}

/** /clear just after this hook, which holds the turn, then send the message to the emptied session. */
function clearFirst($, event) {
  $.clock.after(0, async () => {
    try {
      await $.command.run({ command: "clear", args: "" });
      await $.prompt.submit({ text: event.text });
    } catch (error) {
      $.ui.log(`sessionkit: /clear did not start (${error instanceof Error ? error.message : String(error)})`);
      await $.prompt.fill({ text: event.text });
      $.ui.toast("sessionkit: could not start /clear; your message is back in the prompt box.");
    }
  });
  return { drop: "sessionkit: clearing this session first; your message goes out once that is done." };
}

/**
 * A subagent of this session has grown large while Claude Code has no auto-compact window, so the
 * subagent never compacts. The main session is small and has nothing to /compact: the choice is
 * the window alone (its /autocompact), which the main session shares. The command runs just after
 * this hook, as above, and the message goes out after it.
 */
async function subagent($, event, next, check) {
  const ALWAYS = `Always at ${check.at}`;
  let choice;
  try {
    choice = await $.ui.ask(check.question, { options: [ALWAYS, CONTINUE], header: "Large subagent" });
  } catch {
    choice = CONTINUE; // dismissed
  }
  if (choice !== ALWAYS) return next(event);
  $.clock.after(0, async () => {
    try {
      await $.command.run({ command: "autocompact", args: String(check.window) });
      await $.prompt.submit({ text: event.text });
    } catch (error) {
      $.ui.log(`sessionkit: /autocompact did not run (${error instanceof Error ? error.message : String(error)})`);
      await $.prompt.fill({ text: event.text });
      $.ui.toast("sessionkit: could not set the auto-compact window; your message is back in the prompt box.");
    }
  });
  return { drop: "sessionkit: setting the auto-compact window; your message goes out once that is done." };
}

/** @type {import('claude-code').Register} */
export const register = (on) => {
  on("prompt.submit", async ($, event, next) => {
    // Only what the person typed: a peer's message or a plugin's prompt is not theirs to decide.
    if (event.origin.kind !== "composer" || event.text.trim() === "") return next(event);
    const input = { session_id: await $.session.id(), prompt: event.text };
    let check;
    try {
      check = await sessionkit($, ["cold-check"], input);
    } catch {
      return next(event);
    }
    if (check.large) return large($, event, next, check);
    if (check.subagent) return subagent($, event, next, check);
    if (!check.cold) return next(event);

    // Compact and continue means one thing to the person; how differs: with a handoff, a fresh
    // session continues this one with only what matters; without one, /compact compacts it here.
    // Compact is always offered. A message that looks like new work also gets Clear, first:
    // /clear empties this session in this terminal and sends the message there.
    const here = `Continue here (${check.cost})`;
    const options = check.route === "search" ? [CLEAR, COMPACT, here] : check.fresh ? [COMPACT, here] : [COMPACT, here, STOP];
    let choice;
    try {
      choice = await $.ui.ask(check.question, { options, header: "Cold cache" });
    } catch {
      choice = STOP; // dismissed
    }
    if (choice === CLEAR) return clearFirst($, event);
    if (choice === here || (choice === COMPACT && !check.fresh)) {
      // Without the ack the settings hook stops the message once; sending it again goes through.
      await sessionkit($, ["cold-choice", "continue"], input).catch(() => undefined);
      return choice === here ? next(event) : compactFirst($, event, [["compact", ""]]);
    }
    try {
      if (choice === COMPACT) {
        const answer = await sessionkit($, ["cold-choice", "fresh"], input);
        if (answer.ok) return { drop: "sessionkit: closing this session; your message continues in a fresh one." };
      }
    } catch {
      // Fall through: keep the message, and let the person decide again.
    }
    await $.prompt.fill({ text: event.text });
    return { drop: check.stop };
  });
};
