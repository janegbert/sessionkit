// sessionkit's effort per prompt, in shadow: the person picks an effort level once, and a prompt
// like "yes" or "commit this" then runs at that level. At the first model request of a turn the
// person typed, Jev says which effort level is enough for its prompt, and `sessionkit hook
// effort-route` logs that beside the level the turn runs at, in
// ~/.cache/sessionkit/effort-routes.jsonl. Nothing changes the request, and it does not wait for
// Jev. A subagent's requests are not routed here: agent.js does that when it starts.
//
// With the option applyEffort, the turn runs at Jev's level when that is lower than its own, never
// higher. The first request then waits for the answer, two seconds at most; the level is decided
// once and holds for every request of the turn, and a late or failed answer leaves the turn as it
// was. The prompt cache holds on models with per-turn effort: Anthropic's documentation says a
// change of the request's effort invalidates the cached messages, and that an effort change
// carried inside the conversation keeps them (https://platform.claude.com/docs/en/build-with-claude/effort
// and .../prompt-caching, read 2026-10-02), and Claude Code 2.1.287 sends an effort rewritten here
// in that second form. Measured on a subscription, in one process: on Opus 5.5 the lowered turn
// read 121,367 tokens from the cache and wrote 240, the turn after it read 121,487 and wrote 240;
// Sonnet 5.5 the same at 90K. On a model or provider without per-turn effort a lower effort for
// one turn costs two rewrites of the whole context. Off by default until Jev's answers are measured.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 30_000;
const APPLY_WAIT_MS = 2_000;
const LEVELS = ["low", "medium", "high", "xhigh", "max"];

const typed = new Set(); // prompts the person typed whose turn has not started yet
let turn = null; // the turn of such a prompt: its id and prompt, and from its first request the effort to apply

/** Jev's effort level for the prompt of a turn, logged by sessionkit; null when there is none. */
async function route($, event, prompt, timeoutMs) {
  try {
    const input = { session_id: await $.session.id(), turn_id: event.turnId, prompt, model: event.model, effort: event.effort };
    const run = await $.process.run([SESSIONKIT, "hook", "effort-route"], { stdin: JSON.stringify(input), timeoutMs });
    return JSON.parse(run.stdout).proposal?.effort ?? null;
  } catch (error) {
    $.ui.log(`sessionkit: effort route not logged (${error instanceof Error ? error.message : String(error)})`);
    return null;
  }
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  // Only what the person typed: a peer's message or a notification is not a prompt to route.
  on("prompt.submit", { origin: { kind: "composer" } }, ($, event, next) => {
    if (event.text.trim() !== "") typed.add(event.text);
    return next(event);
  });

  on("turn.start", ($, event, next) => {
    if (typed.delete(event.text)) turn = { id: event.turnId, prompt: event.text };
    return next(event);
  });

  on("turn.step", async function* ($, event, next) {
    // A subagent's request, a turn nobody typed and a model without effort stay as they are.
    if (event.agentId || event.turnId !== turn?.id || event.effort === undefined) return yield* next(event);
    if (!("effort" in turn)) {
      turn.effort = null;
      const answer = route($, event, turn.prompt, options?.applyEffort ? APPLY_WAIT_MS : TIMEOUT_MS);
      if (options?.applyEffort) turn.effort = await answer;
    }
    const lower = LEVELS.includes(turn.effort) && LEVELS.indexOf(turn.effort) < LEVELS.indexOf(event.effort);
    return yield* next(lower ? { ...event, effort: turn.effort } : event);
  });
};
