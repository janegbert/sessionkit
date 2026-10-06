# First-class code navigation tools

## Goal and design

Agents should discover SessionKit before doing speculative keyword searches or
reading files one by one. The installed skill and the existing reminder after a
large read remain useful, but the reminder arrives after the context cost.

`plugin/hooks/search-tools.js` exposes two tools via `$.tool.register`, awaiting
registration during session.start:

- `mcp__sessionkit__search_code`: behavior-oriented repository search over the
  existing `sessionkit grep`; question, optional root and source-output budget.
- `mcp__sessionkit__ask_file`: facts about a known file over `sessionkit ask`;
  batches 1..10 questions, or mode=find for one question.

Both are kept in the initial tool list through tool.describe (`isDeferred:
false`), rather than requiring agents to discover them via ToolSearch. This adds
schema overhead to each prompt; it needs to earn that cost through less tool
output. It is not automatically a net token saving on every task.

A stable, short prompt.compose section supplies the navigation decision rule
only when registration succeeded and the prompt offers both tools. agent.spawn
adds the same conditional rule once to delegated tasks. Spawn inputs do not
expose the child's actual tool allowlist, so the instruction says "when these
tools are available" and never changes an agent definition or tool allowlist.
Agents with restrictive configurations may not inherit the tools.

`codeSearchTools` is on by default, with an off switch in plugin options. The
existing CLI skill still works without function hooks. setup embeds the runtime
module and replaces its executable placeholder with the installed binary path.
The setup file inventory also includes the new permission modules imported by
the current index; otherwise a newly installed plugin would fail to load.

## Boundaries

- Never replace a built-in search/read call with a paid call automatically.
- Exact names, literals and paths remain for ordinary Grep/Glob/Read.
- Tool descriptions disclose eligible source is sent to TypeSafe and Jev calls
  are paid. Authentication uses the existing CLI, not a new credential store.
- Expose no sensitive/hidden/ignored/dependency inclusion flags. Preserve the
  CLI defaults. Search uses `--` before question/root. File paths for ask are
  absolute and its questions cannot be CLI flags. No shell is used.
- Search returns a default source budget of 12,000 bytes, adjustable to
  1..100,000. This is not a completeness guarantee and does not bound every byte
  of the CLI's metadata output. The process API separately caps stdout/stderr.
- Preserve stdout and diagnostics on incomplete discovery (exit 2), failures,
  interruption and process-output truncation. Tell agents missing evidence is
  unknown, with ordinary Grep/Read as fallback. Never automatically retry a paid
  search. Processes have a two-minute timeout.
- Read excerpts first; do not automatically open every ranked file. Read missing
  context before editing. Probabilities and file relevance are not proof.
- Relative paths/root use the session cwd. Delegation into another cwd should
  supply explicit search roots or absolute file paths.

## Verification

`claude plugin test plugin`: 82 tests pass, including nine new tests covering
native tool dispatch, CLI argv, batching/find, option injection, opt-out,
registration failures, stable/deduplicated guidance, input bounds, partial
results, truncation and process errors. `cargo test`: 92 tests pass, including
setup's inventory check that all runtime plugin files are embedded.

Two live tests on Claude Code 2.1.291 used only an artificial two-file Python
fixture under `/tmp/sessionkit-search-tools.xU7dSu/fixture`:

1. The main agent called search_code and got the implementation and test
   excerpts. A general-purpose Sonnet subagent called ask_file and got a
   probability with a supporting line. Both invoked the real installed CLI.
   The supporting line was a docstring rather than the exception handler; the
   agent correctly noted that this was indirect evidence.
2. A Sonnet Explore subagent received a task and fixture directory but **no
   tool name or search strategy in the parent's delegation**. With the plugin's
   navigation rule it chose search_code once and answered from its excerpts,
   without direct file reads. The stream contains the tool call with the Agent
   call's `parent_tool_use_id`, independently of the agent's self-report.

Artifacts: `output.jsonl`, `debug.log`, `explore-output.jsonl` and
`explore-debug.log` in the temporary directory above. Both tests used a temporary
inline plugin; no installed plugin or user configuration was updated. Claude
reported costs of about $0.16 and $0.15, respectively; those are Claude costs,
not combined Claude + Jev accounting.

This proves visibility, dispatch and one small unprompted tool-choice case. It
does **not** establish improved adoption across projects, completeness, latency
or net cost/token savings. No automatic search-pattern intervention is included.

## Next measurement

Compare the current skill/read-tip baseline to tools + the short decision rule
on behavior searches, literal searches, known-file questions and broad edits.
Measure main/subagent tool selection separately, correct file discovery,
search/read call counts, tool-output characters, prompt/schema overhead, wall
time and combined Claude/Jev cost. Include auth failures and incomplete searches.
Do not count greater SessionKit usage as success if total cost or quality worsens.

References:
- https://code.claude.com/docs/en/plugins/mods/api#add-a-tool
- https://code.claude.com/docs/en/plugins/mods/events
- Claude Code 2.1.291's generated declarations, especially ToolSpec,
  ToolDescribeResult, PromptComposeInput and AgentSpawnInput.
