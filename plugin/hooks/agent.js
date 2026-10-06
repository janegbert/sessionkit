// sessionkit's model routing for subagents, in shadow: after a subagent starts, Jev says which
// model and effort level would have been enough for its task, and `sessionkit hook agent-route`
// logs that beside the model it got, in ~/.cache/sessionkit/agent-routes.jsonl. Nothing changes
// the spawn, and it does not wait for Jev. The log is for a measurement, before any routing is
// switched on.
//
// sessionkit setup writes this file with the path of sessionkit on this machine.

const SESSIONKIT = "__SESSIONKIT__";
const TIMEOUT_MS = 30_000;

/** @type {import('claude-code').Register} */
export const register = (on) => {
  on("agent.spawn", async ($, event, next) => {
    const result = await next(event);
    if (!result.agentId) return result;
    const input = {
      session_id: await $.session.id(),
      agent_id: result.agentId,
      agent_type: event.subagentType,
      description: event.description,
      prompt: event.prompt,
      model: result.model,
      requested: event.model ?? null,
      parent_model: event.parentModel,
    };
    $.process.run([SESSIONKIT, "hook", "agent-route"], { stdin: JSON.stringify(input), timeoutMs: TIMEOUT_MS })
      .catch((error) => $.ui.log(`sessionkit: agent route not logged (${error instanceof Error ? error.message : String(error)})`));
    return result;
  });
};
