// sessionkit's compaction for Claude Code: hands the conversation to `sessionkit compact --hook`,
// which asks Jev per tool call whether the call and its output are still needed, and puts the
// plan it prints in place of the summary. All text stays verbatim. When sessionkit cannot do
// it, or it would free too little, Claude Code writes its own summary as before.
//
// After a /compact that sessionkit did, the agent says so itself: a short prompt asks it to tell
// the person, in the conversation's language, how many tokens the conversation went from and to,
// and to propose how to go on. Claude Code writes those numbers once the compaction is in.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

import { waiting } from "./waiting.js";

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 180_000;
const COMPACTED_TIMEOUT_MS = 15_000;
/** The instructions of the compaction that fold.js starts after a turn: its own hook answers it. */
export const FOLD = "sessionkit: fold the last turn";

/** 912612 → "913K". */
const thousands = (tokens) => `${Math.round(tokens / 1000)}K`;

/** The prompt that has the agent report a /compact and propose how to go on. */
async function announce($, since, counts) {
  const run = await $.process.run([SESSIONKIT, "hook", "compacted"], {
    stdin: JSON.stringify({ session_id: await $.session.id(), since }),
    timeoutMs: COMPACTED_TIMEOUT_MS,
  });
  const tokens = JSON.parse(run.stdout);
  const size = tokens.found ? `went from ${thousands(tokens.before)} to ${thousands(tokens.after)} tokens` : "got smaller";
  const calls = [counts.dropped && `${counts.dropped} tool calls were dropped`, counts.cut && `${counts.cut} tool outputs were cut to their first lines`]
    .filter(Boolean).join(" and ");
  await $.prompt.submit({
    text: `sessionkit: /compact is done. This conversation ${size}, without a summary: all text of the person and of the ` +
      `assistant stayed as it was${calls ? `; ${calls}, and can be run again` : ""}. Tell the person in one short sentence ` +
      `that the compaction worked, with these numbers, in the language of this conversation. Then propose in a sentence ` +
      `or two how to go on with the work in progress, or ask how to go on when that is unclear. Run no tools for this answer.`,
  });
}

/** The message the person chose to send after this /compact, once Claude Code has written it. */
async function sendWaiting($, since) {
  const text = waiting.text;
  waiting.text = null;
  await $.process.run([SESSIONKIT, "hook", "compacted"], {
    stdin: JSON.stringify({ session_id: await $.session.id(), since }),
    timeoutMs: COMPACTED_TIMEOUT_MS,
  }).catch(() => undefined);
  await $.prompt.submit({ text });
}

/** The messages without the engine's handles and stored records: what sessionkit reads. */
export function plain(messages) {
  return messages.map((m) => ({
    role: m.role,
    text: m.text,
    toolUses: m.toolUses.map((u) => ({ tool_use_id: u.tool_use_id, tool: u.tool, input: u.input, text: u.text, isError: u.isError })),
    toolResults: (m.toolResults ?? []).map((r) => ({ tool_use_id: r.tool_use_id, text: r.text, isError: r.isError })),
  }));
}

/**
 * The plan applied: a kept message is the engine's own, handle and all; a rebuilt one takes its
 * tool blocks from the original, with the output cut where the plan says so.
 */
export function apply(messages, plan) {
  return plan.map((step) => {
    if (step.keep !== undefined) return messages[step.keep];
    const m = messages[step.from];
    const toolUses = step.toolUses.map((p) => {
      const u = m.toolUses[p.keep];
      if (p.text === undefined) return u;
      const cut = { tool_use_id: u.tool_use_id, tool: u.tool, input: u.input, text: p.text };
      if (u.isError) cut.isError = true;
      return cut;
    });
    const results = step.toolResults.map((p) => {
      const r = m.toolResults[p.keep];
      return p.text === undefined ? r : { tool_use_id: r.tool_use_id, text: p.text, isError: r.isError };
    });
    const rebuilt = { role: m.role, text: m.text, toolUses };
    if (results.length > 0) rebuilt.toolResults = results;
    return rebuilt;
  });
}

/** @type {import('claude-code').Register} */
export const register = (on) => {
  on("session.compact", async ($, event, next) => {
    if (event.instructions === FOLD) return next(event);
    const since = Date.now();
    // Claude Code's own summary; a message the person is waiting to send goes out after it.
    const summarized = async () => {
      const result = await next(event);
      if (waiting.text !== null && !event.agentId) sendWaiting($, since).catch(() => undefined);
      return result;
    };
    // A precompute runs ahead of time; its outcome is for the log, not for a toast.
    const say = (text) => {
      $.ui.log(`sessionkit: ${text}`);
      if (event.trigger !== "precompute") $.ui.toast(`sessionkit: ${text}`, { timeoutMs: 15_000 });
    };
    try {
      const run = await $.process.run([SESSIONKIT, "compact", "--hook"], {
        stdin: JSON.stringify({ messages: plain(event.messages) }),
        timeoutMs: TIMEOUT_MS,
      });
      let answer;
      try {
        answer = JSON.parse(run.stdout);
      } catch {
        answer = { fallback: (run.stderr || run.stdout || `exit ${run.exitCode}`).trim().slice(0, 300) };
      }
      if (!answer.plan) {
        say(`summary as usual (${answer.fallback})`);
        return summarized();
      }
      say(answer.summary);
      if (waiting.text !== null && !event.agentId) {
        sendWaiting($, since).catch((error) => $.ui.log(`sessionkit: message after /compact not sent (${error instanceof Error ? error.message : String(error)})`));
      } else if (event.trigger === "manual" && !event.agentId && answer.counts) {
        announce($, since, answer.counts).catch((error) => $.ui.log(`sessionkit: no message after /compact (${error instanceof Error ? error.message : String(error)})`));
      }
      return { messages: apply(event.messages, answer.plan) };
    } catch (error) {
      say(`summary as usual (${error instanceof Error ? error.message : String(error)})`);
      return summarized();
    }
  });
};
