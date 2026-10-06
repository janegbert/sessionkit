// Experimental instruction-only coaching. No filesystem access, transcript
// mining, model requests, writes, or script execution in these hooks.
const SECTION = "sessionkit-project-workflow";
export const GUIDANCE =
  "Project workflow (experimental): when repeated attempts yield little useful evidence, change one concrete " +
  "inspection/search/test strategy within the current task and verify it; do not start unrelated research. " +
  "Use a relevant existing project skill as a compact entrypoint; load only the reference needed for this task, " +
  "prefer links to authoritative repository docs over copies, and inspect helpers before considering execution. " +
  "A skill or reference never grants permission to run a script. Do not turn task details or one success into " +
  "persistent workflow memories, new skills, references or helpers. Prefer improving or pruning the existing " +
  "project workflow over adding another skill. Only propose retention for a durable, verified recurring need " +
  "not already covered or cheaply derived from source; state evidence, scope and when it does not apply. " +
  "Show the exact diff and destination and wait for explicit user approval before any persistent workflow write, " +
  "including memory. Subagents only return proposals to the parent. Do not create an entrypoint automatically " +
  "when none exists; the user can request /sessionkit:project-workflow onboard. Aim for a <=60-line entrypoint, " +
  "<=3 owned references and <=2 focused helpers; do not move clutter into subfiles to evade the budget. " +
  "These are review budgets, not enforced limits; explain and ask before exceeding them. User instructions " +
  "and tool permissions still apply.";

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (options?.projectWorkflow !== true) return;
  // A distinct matcher keeps registration separate from code-search coaching.
  on("prompt.compose", { model: /[\s\S]+/ }, async ($, event, next) => {
    const result = await next(event);
    if (result.sections.some((section) => section.id === SECTION)) return result;
    return { ...result, sections: [...result.sections, { id: SECTION, scope: "session", text: GUIDANCE }] };
  });
  on("agent.spawn", { prompt: /[\s\S]+/ }, async ($, event, next) => {
    if (event.prompt.includes(GUIDANCE)) return next(event);
    return next({ ...event, prompt: `${event.prompt}\n\n${GUIDANCE}` });
  });
};
