// Opt-in diagnostic only. `ask` hands the call to the mode's decider,
// including the Auto classifier; it is not a classifier rejection.

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  if (options.permissionProbe !== true) return;
  on("tool.check", async ($, event, next) => {
    const verdict = await next(event);
    try {
      $.ui.log(`permission probe: ${JSON.stringify({ event, verdict })}`, { to: "debug" });
    } catch { /* diagnostics must never change permissions */ }
    return verdict;
  });
};
