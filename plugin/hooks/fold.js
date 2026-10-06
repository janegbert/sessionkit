// sessionkit's fold of a finished turn, behind the option foldTurns. A tool output is read once
// and then rides along in every later call. Cutting it in the middle of a conversation changes
// the request from that point on, so the rest is written to the prompt cache again; right after
// a turn, that turn is the end of the conversation, and everything before it stays cached.
//
// So when a turn ends that added much tool output, this starts a compaction of its own, and
// answers it with the plan of `sessionkit compact --fold`: every message stays as the engine has
// it, except the tool results of that turn, cut to their first lines. The prompt, the
// assistant's text and thinking and every call with its input stay; a call that changed a file
// keeps its output too. No model is asked and nothing is summarized.
//
// Measured with Claude Code 2.1.287 and Haiku, on a turn that read a file of 22K tokens and ended
// at 74.5K tokens of context: the first call of the next turn read 51.6K from the cache and wrote
// 0.8K, where it would have read 74K. A plugin can start a compaction only at turn.complete and
// only in an interactive session; in a headless one this does nothing.
//
// A fold is a compaction to Claude Code: the PreCompact and SessionStart hooks of the settings
// run, the screen shows "Conversation compacted", and the transcript file gets one more copy of
// the conversation.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

import { FOLD, apply, plain } from "./compact.js";

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 15_000;
// The fold asks no model. What it costs comes at the next call: what is left of the turn after
// its first cut output is written to the cache again instead of read (a write is 2 times the
// input price with the one-hour cache, a read 0.1 times), and the transcript file gets one more
// copy of the conversation. What it saves is 0.1 times the cut tokens at every later call, and
// all of them at every rebuild of the cache. 20,000 characters are about 5,000 tokens: against
// a rest of the turn of some 2,500 tokens that is paid back after ten calls.
const MIN_RESULT_CHARS = 20_000;

/** The characters of tool output the last turn added: everything after the person's last prompt. */
function resultChars(messages) {
  const start = messages.findLastIndex((m) => m.role === "user" && m.text.trim() !== "" && !m.toolResults?.length);
  return messages.slice(start + 1).reduce((sum, m) => sum + (m.toolResults ?? []).reduce((n, r) => n + r.text.length, 0), 0);
}

const reason = (error) => (error instanceof Error ? error.message : String(error));

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (!options?.foldTurns) return;

  on("turn.complete", { reason: "answer" }, async ($, event, next) => {
    const result = await next(event);
    if (event.agentId || event.reason !== "answer") return result;
    try {
      if (resultChars(await $.session.messages()) >= MIN_RESULT_CHARS) await $.session.compact({ instructions: FOLD });
    } catch (error) {
      $.ui.log(`sessionkit: turn not folded (${reason(error)})`);
    }
    return result;
  });

  // A fold that fails leaves the conversation as it is: Claude Code's summary is no fallback here.
  on("session.compact", { trigger: "plugin" }, async ($, event, next) => {
    if (event.instructions !== FOLD) return next(event);
    try {
      const run = await $.process.run([SESSIONKIT, "compact", "--fold"], {
        stdin: JSON.stringify({ messages: plain(event.messages) }),
        timeoutMs: TIMEOUT_MS,
      });
      const answer = JSON.parse(run.stdout);
      if (!answer.plan) return { skip: `sessionkit: turn not folded (${answer.fallback})` };
      $.ui.log(`sessionkit: ${answer.summary}`);
      return { messages: apply(event.messages, answer.plan) };
    } catch (error) {
      return { skip: `sessionkit: turn not folded (${reason(error)})` };
    }
  });
};
