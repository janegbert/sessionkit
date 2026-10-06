# Ask after an Auto denial

## User-facing flow

The plugin option **Ask after an Auto denial (experimental)** (`askOnAutoDenial`)
is off by default. With function hooks enabled, it keeps Claude Code's normal
Auto check first. After a recognized Auto classifier denial in the main loop,
SessionKit uses the engine's AskUserQuestion dialog to show the tool, exact
arguments and denial reason, and offers:

- **Keep denied**
- **Allow once**

Only the exact answer `Allow once` authorizes one retry. Closing the dialog,
free text, interruption or an unavailable interactive UI keeps the denial.
SessionKit does not call Jev, read the conversation to infer consent, save
permissions, or generate wildcard rules. A later denied call needs a new answer.

This route is **experimental**: simulated tests and live successful-call smoke
tests pass, but a real classifier-denial retry is not yet proven live. The
installed SessionKit 0.21.0 has not been updated or configured by this change.

## Implementation

`plugin/hooks/auto-permission.js`, registered by `plugin/hooks/index.js`, uses
three function events:

1. `tool.call` tracks the call until its continuation settles.
2. `tool.check` records the actual permission input. An initial `ask` is left
   unchanged so the Auto classifier can decide.
3. `classic.PermissionDenied` records an Auto denial that matches the call ID,
   tool, checked arguments and agent identity.

If the enclosing tool.call returns an error/denial, the hook asks the user.
On approval it invokes the **same continuation** once more. On this retry only,
tool.check consumes the approval once and may replace `ask` with `allow` for
that same ID and identical input. It does not override `deny`, a settings rule,
a classic hook decision, or an organization ceiling. Pending state is removed
when the call settles, including on exceptions. Successful tool results are
never replayed. Subagents and AskUserQuestion are excluded for this first version.

No-verdict failures remain denied without a question: empty reasons,
`Classifier unavailable`, and the documented reason prefix
`Auto mode could not evaluate this action and is blocking it for safety`.

PermissionDenied's own `retry: true` only tells **the model** it may issue a new
call; it does not reverse the denial. SessionKit does not set this field. Its
one-time approval must not transfer to a new model call or a concurrent call.

The old `permissionProbe` option is now strictly diagnostic: it logs tool.check
inputs and verdicts and never changes decisions or asks questions. It is also
off by default. **Privacy:** this diagnostic option logs arguments that may
contain secrets. The approval feature itself does not log denial arguments;
the dialog necessarily displays them to the person being asked.

## Why not review `tool.check` denials directly?

Claude Code 2.1.291's generated types define tool.check as the **declarative**
permission decision. `ask` hands a call to the mode's decider, explicitly
including the Auto classifier. Approving the first ask would skip the classifier
rather than review its rejection. ToolCallResult has no typed rejection-source
field, so an error string alone is not a reliable signal either.

The hooks reference says PermissionDenied fires only for Auto denials, including
no-verdict failures. It does not fire for manual dialog refusals, PreToolUse
blocks or explicit deny rules. The installed build exposes this event as
`classic.PermissionDenied` to function hooks.

## Verification and remaining live check

Run `claude plugin test plugin`. The suite covers opt-in registration, forwarding
PermissionDenied, exact one-time approval, refusal, free text, dismissal,
interruption, changed arguments, explicit rules, hook decisions, ceilings,
no-verdict failures, call isolation, continuation exceptions, successful calls
and excluded subagents. Full retry tests use controlled callbacks; they do not
manufacture a real classifier verdict.

Three live Auto smoke tests on Claude Code 2.1.291 executed only
`printf sessionkit-permission-probe`. The debug log confirmed Auto access and
showed tool.check's unchanged `ask`, followed by successful execution. The second
and third smoke tests loaded all three events; the third used the final module
and option names. None of these tests produced a classifier denial.
Artifacts are in `/tmp/sessionkit-permission-probe.lvWTUZ/`, including
`debug.log`, `result.txt`, `retry-smoke-debug.log`, `retry-smoke-result.txt`,
`approval-smoke-debug.log`, `approval-smoke-result.txt`, and
the declarations generated under `.claude-plugin/types/claude-code/index.d.ts`.

Before normal use, verify with a controlled **harmless** action Auto actually
rejects that PermissionDenied fires before the enclosing continuation settles
and that the same-ID continuation retry is accepted. Check both refusal and
explicit approval, at most one actual execution, and no settings changes. Do
not try a dangerous action merely to induce a classifier rejection.

## Future persisted permissions

Not implemented. One-time approval and saving a rule would require separate
choices. Show a proposed rule and what its wildcards admit before saving; do
not blindly use the exact command (too specific) or the whole executable (too
broad). Preserve safety-relevant arguments and path boundaries where the syntax
can represent them, accounting for shell chains, pipes, redirects and
substitutions. If no safe rule is expressible, retain one-time approval. Saved
allow rules may skip the classifier on future calls.

## References

- https://code.claude.com/docs/en/plugins/mods/reference
- https://code.claude.com/docs/en/plugins/mods/events
- https://code.claude.com/docs/en/plugins/mods/api
- https://code.claude.com/docs/en/hooks#permissiondenied
- Installed build's generated declarations (prefer over the GitHub copy).
