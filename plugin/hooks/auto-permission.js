// Ask the person after an Auto denial; never infer consent or call a model.
// Experimental until a real classifier-denial retry has been verified live.
// PermissionDenied identifies the source; error text from tool.call does not.

const canonical = (value) => JSON.stringify(value, (_key, item) => {
  if (!item || typeof item !== "object" || Array.isArray(item)) return item;
  return Object.fromEntries(Object.keys(item).sort().map((key) => [key, item[key]]));
});
const identity = (tool, input, agentId) => canonical({ tool, input, agentId: agentId ?? null });
const noVerdict = (reason) => typeof reason !== "string" || !reason.trim() ||
  reason.startsWith("Auto mode could not evaluate this action and is blocking it for safety") ||
  reason === "Classifier unavailable";

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (options.askOnAutoDenial !== true) return;
  // Per registration/session. Entries exist only while the enclosing call runs.
  const pending = new Map();

  on("classic.PermissionDenied", async ($, event, next) => {
    const result = await next(event);
    try {
      const state = pending.get(event.tool_use_id);
      if (state?.phase === "initial" && !noVerdict(event.reason) &&
          identity(event.tool_name, event.tool_input, event.agent_id) === state.checked) {
        state.denial = { tool: event.tool_name, input: event.tool_input, reason: event.reason, identity: state.checked };
      }
    } catch { /* invalid denial metadata must not affect the original verdict */ }
    // Do not set retry: that would ask the model for a new call, not retry this one.
    return result;
  });

  on("tool.check", { tool: "*" }, async ($, event, next) => {
    const verdict = await next(event);
    const state = pending.get(event.tool_use_id);
    if (!state) return verdict;
    const checked = identity(event.tool, event.input, event.agentId);
    if (state.phase === "initial") {
      state.checked = checked;
      return verdict;
    }
    // Consume before deciding, never transfer to another call or argument set.
    const approved = state.approved;
    state.approved = null;
    if (approved === checked && verdict.decision === "ask" &&
        !verdict.rule && !verdict.hook && !event.ceiling && !verdict.ceiling &&
        !next.signal?.aborted) return { decision: "allow" };
    return verdict;
  });

  on("tool.call", async ($, event, next) => {
    const id = event.tool_use_id;
    // Exclude subagents until the dialog and PermissionDenied loop identity have
    // been verified live. No dialog recursion and no nested registration for an id.
    if (!id || event.agentId || event.tool === "AskUserQuestion" || pending.has(id)) return next(event);
    const state = { phase: "initial", checked: null, denial: null, approved: null };
    pending.set(id, state);
    try {
      const result = await next(event);
      const denial = state.denial;
      if (!denial || (!result.isError && typeof result.deny !== "string") || next.signal?.aborted) return result;
      // PermissionDenied + a failing result proves this attempt did not run.
      // A successful result must never be replayed, even if a nested call failed.
      let answer;
      try {
        answer = await $.ui.ask(
          `Auto mode denied ${denial.tool}: ${denial.reason}. Arguments: ${JSON.stringify(denial.input)}. ` +
          "Allow this exact action once? This does not save a permission rule.",
          ["Keep denied", "Allow once"],
        );
      } catch { return result; }
      if (answer !== "Allow once" || next.signal?.aborted) return result;
      state.phase = "retry";
      state.approved = denial.identity;
      try { $.ui.log(`Auto denial: one-time retry approved (${id})`, { to: "debug" }); } catch {}
      // Same continuation, same id. Normal checks still run; tool.check may only
      // replace ask for the exact approved input. There is no second retry.
      return await next(event);
    } finally {
      pending.delete(id);
    }
  });
};
