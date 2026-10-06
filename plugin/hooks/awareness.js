// sessionkit's cost awareness, an experiment behind the option costAwareness: what tokens cost,
// in the agent's context at the moments it can act on it, in as few tokens as possible.
//
// At session start `sessionkit hook awareness-start` marks the session, so the settings hook's
// cost note comes with the first message (with the reason why cost matters) and then only when
// the context has doubled. Each subagent gets the reason as the last line of its prompt, and
// one line of numbers each time its own context doubles: its requests are watched here, since
// nothing else tells a subagent what it costs. When a subagent ends, the main session gets one
// line on what it took. `sessionkit hook awareness-agent` words the lines: prices and the
// 5-hour limit live there.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 10_000;

const agents = new Map(); // agent id → { description, noted: context of the last note }
let start = null; // { reason, first_note_at }, once the session is marked

const reason = (error) => (error instanceof Error ? error.message : String(error));

async function sessionkit($, hook, input) {
  const run = await $.process.run([SESSIONKIT, "hook", hook], { stdin: JSON.stringify(input), timeoutMs: TIMEOUT_MS });
  return JSON.parse(run.stdout);
}

/** The line for a subagent whose context passed a doubling, or for the main session when it ended. */
async function line($, input) {
  try {
    const { text } = await sessionkit($, "awareness-agent", { session_id: await $.session.id(), ...input });
    return text || null;
  } catch (error) {
    $.ui.log(`sessionkit: cost note not made (${reason(error)})`);
    return null;
  }
}

async function append($, text, agentId) {
  const message = { type: "user", content: [{ type: "text", text }] };
  await $.session.append({ message, ...(agentId ? { agentId } : {}) })
    .catch((error) => $.ui.log(`sessionkit: cost note not delivered (${reason(error)})`));
}

/** A subagent to watch, from its spawn. */
export function spawned(agentId, description) {
  agents.set(agentId, { description, noted: 0 });
}

/** After a subagent's request: one line when its context has doubled since the last. */
export async function stepped($, agentId, model, usage) {
  const agent = agents.get(agentId);
  if (!agent || !usage || !start) return;
  const context = usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
  if (context < start.first_note_at || context < 2 * agent.noted) return;
  agent.noted = context;
  const text = await line($, { kind: "grown", agent_id: agentId, context, model });
  if (text) await append($, text, agentId);
}

/** When a subagent ends: what it took, for the main session. */
export async function ended($, agentId) {
  const agent = agents.get(agentId);
  if (!agent) return;
  agents.delete(agentId);
  const text = await line($, { kind: "ended", agent_id: agentId, description: agent.description });
  if (text) await append($, text);
}

/** For tests: forget the session. */
export function reset(marked = null) {
  agents.clear();
  start = marked;
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (!options?.costAwareness) return;

  on("session.start", { cwd: /./ }, async ($, event, next) => {
    try {
      start = await sessionkit($, "awareness-start", { session_id: await $.session.id() });
    } catch (error) {
      $.ui.log(`sessionkit: cost awareness not started (${reason(error)})`);
    }
    return next(event);
  });

  // Every spawn; the matcher only sets this hook apart from the plugin's others on agent.spawn.
  on("agent.spawn", { prompt: /[\s\S]/ }, async ($, event, next) => {
    if (!start) return next(event);
    const result = await next({ ...event, prompt: `${event.prompt}\n\n${start.reason}` });
    if (result.agentId) spawned(result.agentId, event.description);
    return result;
  });

  on("turn.step", { agentId: /./ }, async function* ($, event, next) {
    for await (const chunk of next(event)) {
      if (chunk.kind === "stop") $.clock.after(0, () => stepped($, event.agentId, event.model, chunk.usage));
      yield chunk;
    }
  });

  on("turn.complete", { agentId: /./ }, async ($, event, next) => {
    const result = await next(event);
    $.clock.after(0, () => ended($, event.agentId));
    return result;
  });
};
