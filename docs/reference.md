# SessionKit reference

Detailed command and integration documentation. For an overview and installation,
start with the [README](../README.md). Claude Code is the primary integration;
Codex and jcode currently have launcher support only.

- **`sessionkit start`** starts Claude Code, Codex or jcode with the relevant parts of earlier
  Claude Code sessions. A long session with a cold cache is expensive to resume, and every
  further call re-reads its whole context. `start` begins fresh and hands over only what
  matters for your first prompt. [Jev](https://docs.typesafe.ai) (TypeSafe) selects the parts;
  it never writes or summarizes, so everything kept stays verbatim.
- **`sessionkit usage`** shows where the tokens and the money go: by kind of token, by model,
  main sessions against subagents, by project, the largest sessions, cold cache rebuilds, and
  sessions that are cheaper to restart than to continue. Local and free.
- **`sessionkit grep`** finds code by asking what it does: Jev judges folders, files and
  declarations for a question, and the answer lists the relevant files with verbatim source,
  for a coding agent to read. A port of [jevgrep](https://github.com/dzhng/jevgrep).
- **`sessionkit ask`** answers a question about a file without reading it into the agent's
  context: up to ten facts about one file, one question over many files, or the line that
  answers it. A port of the ask-jev plugin.
- **`sessionkit compact`** compacts a Claude Code conversation without a summary, from
  `/compact` and auto-compaction: Jev decides per tool call whether it and its output are still
  needed, and all text stays verbatim. A port of
  [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction), with a measured
  question about the output.
- **`sessionkit skills`** finds work you repeat across sessions, where a skill would save time:
  alike exchanges are grouped on your machine, Jev judges whether each group repeats one task
  and whether an installed skill already covers it, and `--draft` starts Claude Code to write
  the skill.
- **`sessionkit measure`** measures whether `start` selects the right context.
- **`sessionkit statusline`** and **`sessionkit hook`** show inside Claude Code what the prompt
  cache costs: the context, the cost of each call, how long the cache stays warm, and a warning
  before a cold call rewrites a large context. **`sessionkit next`** closes a cold session from
  inside Claude Code and continues it in a fresh one.

## Compact project workflow (experimental)

The `projectWorkflow` plugin option defaults off. It adds short, stable coaching
for the main agent and subagents: adapt within the task, reuse one project
workflow entrypoint, load only needed references and favor canonical docs over
copies. Durable workflow changes require a precise proposal and explicit approval.

`/sessionkit:project-workflow onboard`, `review` and `propose <lesson>` are explicit
proposal workflows, available even with coaching disabled. Default review budgets
are 60 entrypoint lines, three owned references and two helpers, with justified
exceptions; these are not hard enforcement. Nothing is automatically saved or
executed by the hooks. Native memory writers and other features remain independent.
See [project-workflow design, usage and limits](project-workflow.md).

## Install

The tested installation path is macOS with a current Rust toolchain and Claude Code:

```sh
git clone https://github.com/janegbert/sessionkit.git
cd sessionkit
cargo install --path . --locked
sessionkit setup
```

`cargo build --release` instead puts the binary in `target/release/sessionkit`.
A public Homebrew tap is not part of this installation path. For `start`, the chosen
agent (`claude`, `codex` or `jcode`) must be installed separately.

From the npm version: run `npm uninstall -g sessionkit`, then `sessionkit setup` again.
Setup replaces the entries that pointed to the npm install.

Jev commands (including `start`, `measure`, `grep` and `ask`) need your own TypeSafe key.
Run `sessionkit auth login` to store it in the macOS Keychain, or set `TYPESAFE_API_KEY`. `usage` and the status line need no key and no network. Jev-backed plugin features,
such as compaction, architecture checks and the code-search tools, do need a key
and make network requests when used.

## sessionkit usage

```sh
sessionkit usage                     # everything
sessionkit usage --days 7            # the last week
sessionkit usage --project myproject # one project
sessionkit usage --json              # the numbers as JSON
sessionkit usage --tools             # which tools fill the context
```

It reads the transcripts under `~/.claude/projects`, main sessions and the subagents stored
beside them, and adds up the usage every API call records: uncached input, cache reads, cache
writes (5 minutes and 1 hour) and output. A response written as several lines, or a call a
forked session copied, is counted once.

Costs are API-equivalent: token counts times the published per-token prices, including the
lower cache-read prices of Claude Opus 5.5 and Claude Fable 5.1 and double prices for fast
mode. With a subscription you pay a flat fee instead, so read them as the value of what you
used. A model without a known price is named, and its tokens are left out of the costs.

Claude Code's own `cost-state` per session is not used. It counts per process, so a resumed
session starts again at zero; compared with the transcripts it came out both too low and too
high.

**`--tools`** shows which tools fill the context. Per tool: how many results, their size in
tokens (the text at 4 characters a token; each image at the visual tokens Claude counts for its
size, one per 28x28 pixels after the resize of its model's tier), the part of the text that came
in results above 10k and 20k characters, and **re-read**: each result times the API calls after it, up to
the next compaction. Every such call reads the result again from the cache, so re-read is where
the cache reads go. MCP tools appear by their server's name. The output holds only tool names and
numbers, so you can share it as it is.

```text
tool                      results   tokens  share    avg  >10k  >20k   re-read
------------------------------------------------------------------------------
Bash                       17,093     8.2M    60%    481   30%   10%       75%
Read                        2,986     4.0M    29%     1K   67%   49%       15%
mcp: playwright             2,068     341K     2%    165    9%    4%        3%
```

**Cold cache rebuilds** are a heuristic: a call after a pause longer than the cache lifetime
(5 minutes, or 1 hour when the call writes a 1-hour cache) that writes at least 20k tokens to
the cache and reads less than it writes.

**Cheaper to restart** lists sessions active in the last 14 days whose last context is above
150k tokens. Per session it shows what one more call costs in cache reads now, the same after
a fresh start of about 15k tokens, and the one-time write of a cold `--resume`, with the
`sessionkit start` command to restart it.

## In Claude Code: status line and hooks

Claude Code does not show that a session with 800k tokens of context re-reads all of them on
every call, or that the first message after an idle hour writes them all to the cache again.

With an API key it shows dollars:

```text
context 475K · $0.095/call · cache warm 40m (cold: $3.80)
context 705K · cache cold: next message ≈ $5.64 · cheaper: sessionkit start --session 9d17b657
```

On a Pro or Max subscription a call costs a share of your usage limits, so it shows those as
well, next to the API value of the tokens:

```text
context 475K · $0.095/call · 5h 23% (reset 14:10) · 7d 41% (reset in 3d 4h) · ≈ +0.6%/call · cache warm 40m (cold: $3.80)
```

The limits come from Claude Code itself (`rate_limits`). **What one call takes** is an estimate
that sessionkit learns on your machine, because Anthropic does not publish how tokens map to
the limits. After every call it notes how much the 5-hour limit rose and how large the context
was; the estimate is the total rise divided by the total context over your last 100 warm calls,
times the current context. It appears after 5 calls. Other sessions running at the same time
raise the limit too, so the estimate tends to be high; calls that rewrote a cold cache are kept
apart.

- **Status line.** It uses what Claude Code hands it (`prompt_cache`, `context_window`,
  `rate_limits`) and the end of the transcript before the first response. It turns yellow in the last 10 minutes of
  the cache and red when the cache has expired. With `refreshInterval` it counts down.
- **On `--resume` or fork** (`hook session-start`): when the cache has expired and writing it
  again costs more than $1, it says so before the first message, with Claude Code's own
  estimate (`estimated_cache_write_usd`).
- **Before a message to a cold session**: when writing the context again costs more than $2,
  the message does not go out until you choose. A warning would come too late, because the call
  would already run. Claude Code's own question dialog asks, with the cost in the question:

  ```text
  The prompt cache of this session has expired. This message writes 352K tokens to the cache
  again: about $2.82, and every later message re-reads them ($0.070 each). …
  ❯ Compact and continue
    Continue here ($2.82)
  ```

  **Compact and continue** keeps only what matters for your message, so the cache write is
  small. Where it can, it closes this session and continues in a fresh one:
  `sessionkit start --session <this session> "your message"`; elsewhere (see below) it runs
  /compact here and then sends your message, and the choice also offers **Stop**. When Jev finds
  the message clearly new work rather than a continuation (the route question of `start`, 0.7 or
  more for a new topic), the question says so and offers **Clear and continue** instead: it runs
  /clear in this session and then sends your message, so nothing of the old context is written
  again. **Continue here** sends it. Stop, or closing the dialog, puts your message back in the prompt box. The dialog is the engine's,
  so asking costs no model call. It comes from the plugin that `setup` installs, a function hook
  on `prompt.submit`, which needs `CLAUDE_CODE_ENABLE_FUNCTION_HOOKS=1`. Without it, the settings
  hook (`hook prompt`) acts alone: where it can it closes the session and continues in a fresh
  one, elsewhere it stops the message once; send it again to go ahead.

- **With each message, a line on what calls cost**: above 50K tokens of context
  (`SESSIONKIT_COST_NOTE_ABOVE`), the prompt hook adds one line to the agent's context: how
  large its context is and what each of its calls costs, and the same for up to three other
  sessions running in this folder, with whether they are idle and warm. Facts only, no advice;
  about 50 tokens a message.

- **Cost awareness (experimental, plugin option *Cost awareness*, off by default).** What tokens
  cost, at the moments the agent can act on it, in as few tokens as possible:
  - once per session, with the first message, and once per subagent, at the end of its prompt,
    why it matters (about 45 tokens): the budget per period is limited, and every call costs
    money, energy and water; work efficiently, not sparingly, quality first. Energy and water
    are named, not quantified: there are no published figures per token.
  - the line on what calls cost comes only when the context has doubled since the last one,
    instead of with every message;
  - a subagent hears its own cost each time its context doubles (from 50K), which nothing else
    tells it: `sessionkit: your context is 120K tokens; each of your calls reads all of it, about $0.04.`
  - when a subagent ends, the main session hears what it took: `sessionkit: the subagent "Read
    three files" ended: 3 model calls, 219K tokens, about $0.32; its context peaked at 121K.`
  Where the status line has learned what a call takes from your 5-hour limit, the share of the
  limit stands beside the dollars. A line that would reach a subagent after its last request is
  lost: it has no next request to read it in.

- **When a warm session has grown large** (375K tokens by default; `SESSIONKIT_COMPACT_ABOVE`),
  the dialog asks whether to compact first: every message re-reads the whole context, and it
  shows the cost per call now and after /compact. A simulation over earlier sessions found that
  compacting at 300K saves about half of the cache reads, more than acting on a cold cache alone.
  **Compact and continue** runs /compact and then sends your message; **Always at 375K** runs Claude
  Code's own `/autocompact 375000` as well, so it compacts by itself from then on, also in auto
  mode, where no new message comes in to ask at. It asks once per size, and again only when the
  context has grown by half; not at all once you set `autoCompactWindow` yourself.
- **When a subagent has grown large** (the same line, `SESSIONKIT_COMPACT_ABOVE`) while the main
  session is still small and no `autoCompactWindow` is set, the dialog asks whether to set one.
  Without a window a subagent never compacts, and Claude Code has no window per subagent: the
  main session shares it, and both compact at about the window minus 33K. **Always at 375K**
  runs `/autocompact 375000` and then sends your message; **Continue** sends it as it is. The
  window reaches subagents started after it. It asks once per session.

- **An output style to pick, if you like it**: `/output-style sessionkit-robot`. Terse and
  neutral, with no personality, and after a tool call that taught it something the agent writes
  one `Noted:` line with the exact paths, names, error lines and decisions. A compaction without
  summary keeps all text, so those facts survive it while the tool output goes. Measured on the
  transcripts here: of the facts an agent used after a possible compaction, 8.5% were in its own
  text, 51% in a tool input that stays, and 41% only in tool output that would go. Not forced:
  sessionkit works the same with any style. `sessionkit trim --articulation --days 7` measures
  it again for your own sessions, locally and free, with the number of `Noted:` lines per tool
  call as what the notes cost.

- **After the first large read** (400 lines or more, the whole file): one line in the context
  tells the agent that `sessionkit ask` answers a question about a file without reading it, and
  `--find` names the line to read. Agents read whole files out of habit; a skill alone did not
  change that. Before this tip, 1 to 2% of the searches and reads outside this repository went
  through Jev, with ask-jev as MCP tools as well as with the skill. Once per session and per
  subagent, so it costs a few hundred tokens; the read itself goes ahead unchanged. A function
  hook on `tool.call` in the plugin, like the dialog above.

  **Outline first** is an option, off by default: turn on *Outline first for large files* in
  `/config`. Then the first whole read of such a file shows lines 1 to 20 and, below them, every
  declaration and heading with its line number (`sessionkit trim --declarations`), and the agent
  reads the parts it needs with offset and limit. A second whole read of the same file gets all
  of it. Claude Code has read the whole file either way, so an Edit of it is allowed. Measured
  with `sessionkit trim --outline` on 310 whole reads of large files: the outline is a median 9%
  of the file, and the outline plus the parts used afterwards 12% of it. That measure misses a
  read done only to understand the code, so try it before you trust it.

- **When an Edit is refused because the file has not been read**: Claude Code allows an Edit
  only of a file that a Read in this session has seen. A file read with `cat` or `sed`, or before
  a compaction, does not count, and in the transcripts here that was 1.1% of all edits; mostly
  the agent then read the whole file again. The plugin does the Read itself, through the engine,
  so the file counts as read without its text entering the conversation, and tries the Edit
  again. The Edit stays safe: its `old_string` must still match the file exactly, once. Not for
  `replace_all`, nor for Write, which replaces the whole file. A function hook on `tool.call`.

- **After an Auto mode denial (experimental, off by default).** Enable *Ask after an
  Auto denial* in the plugin options (`askOnAutoDenial`) to ask whether to **Allow
  once** or **Keep denied**, showing the tool, arguments and denial reason. Auto's
  normal check runs first; SessionKit never infers approval from earlier messages
  or asks Jev. Approval is limited to the same call and exact arguments, with at
  most one retry and no saved permission or wildcard rule. Closing the dialog,
  interruption or an unavailable interactive UI keeps the denial. Explicit rules,
  hook decisions and organization ceilings remain in force. Main-loop calls only;
  no-verdict classifier failures and subagents are excluded. Requires function
  hooks. **A retry after a real classifier denial has not yet been verified live**;
  see [the implementation and verification notes](auto-permission-probe.md).

- **When a teammate's report arrives twice**: a teammate sends its report with SendMessage, and
  when it goes idle its notification carries the same text again. The lead reads it twice and
  often spends a turn saying it already had it: in the transcripts here, 101 of 616 idle
  notifications with a result repeated a message from the same teammate, and 75 were followed
  by a turn without tools, at a median context of 371K tokens. The notification still arrives,
  so a lead that waits for it still wakes; only a result that shares 60% of its words with one of
  the teammate's last five messages becomes one line naming that message. A function hook on
  `session.receive`, tested against the engine's test kit but not yet in a live team.

- **When a subagent starts**, in shadow: Jev says which model and effort level would have been
  enough for its task, and `hook agent-route` logs that beside the model it got, in
  `~/.cache/sessionkit/agent-routes.jsonl`. Nothing changes the spawn, and it does not wait. The
  choices are the Agent tool's model aliases (haiku, sonnet, opus, fable) and effort levels,
  described in Anthropic's own words from its model selection matrix and effort table; no hook
  can list them, so a new model needs a line in the code. Two Jev questions per subagent, about a
  twentieth of a cent. The log is for a measurement before any routing is switched on. A
  function hook on `agent.spawn`; without a TypeSafe key in the environment or the keychain it
  logs nothing, since a hook must not raise a 1Password prompt.
- **When you send a prompt**, in shadow: you pick an effort level once, and a prompt like "yes"
  or "commit this" then runs at that level. At the first model request of a turn you typed, Jev
  says which effort level is enough for the prompt, seeing also the last thing the agent said,
  and `hook effort-route` logs that beside the level the turn ran at, in
  `~/.cache/sessionkit/effort-routes.jsonl`. Nothing changes the request, and it does not wait.
  The levels are described in Anthropic's own words from its effort table. One Jev question per
  typed prompt; subagents, and models without an effort level, are left alone. Function hooks on
  `prompt.submit`, `turn.start` and `turn.step`; without a TypeSafe key at hand it logs nothing.

  **Applying it** is an option, off by default: turn on *Lower the effort per prompt* in
  `/config`. Then the turn runs at Jev's level when that is lower than its own, never higher;
  the first request waits for the answer for two seconds at most, and the level holds for the
  whole turn. The prompt cache holds: Anthropic documents that a change of a request's effort
  invalidates the cached messages unless it is carried inside the conversation, and Claude Code
  sends a hook's change that way on Opus 5.5, Sonnet 5.5 and Fable 5.1 with an API key or a
  subscription. Measured with Claude Code 2.1.287 on Opus 5.5: the lowered turn read 121.4K
  tokens from the cache and wrote 0.2K, and so did the turn after it. On other models and
  providers one lowered turn costs two rewrites of the whole context. It is off until Jev's
  answers on real prompts are measured.

### Continue in a fresh session from inside Claude Code

When a message goes to a cold, expensive session and you choose a fresh session, the prompt
hook closes it and continues the message in a fresh session, in the same terminal. You can also do this yourself, for instance
on a warm session, with `sessionkit next` and Claude Code's `!` prefix:

```text
! sessionkit next "go on with the staging migration"
```

`next` closes this session and continues it in a fresh one:
`sessionkit start --session <this session> "go on with the staging migration"`, in the same
terminal. While the cache is still warm it refuses, because the next message there is cheap;
add `--force` to start fresh anyway.

A hook or a `!` command runs without the terminal, so it cannot open a new interactive session
itself. Something that owns the terminal must do it once Claude Code has exited. The hook and
`next` leave the prompt in `~/.cache/sessionkit/next/` and stop Claude Code. Then one of two
owners opens the next session:

- the `sessionkit start` that launched this Claude Code;
- else, in zsh, a hook that `sessionkit setup` adds to `~/.zshrc`. Before the next shell prompt
  it finds the prompt for this terminal. It only tests for a file, so your prompt stays fast.

Without either, the hook stops the message once, and `next` prints the command to run.

### Hand work to a warm session

An idle session whose cache is still warm holds context you already paid for. `sessionkit peers`
lists the Claude Code sessions running on this machine, from `~/.claude/sessions`, with their
context, what one call costs in cache reads, and how long the cache stays warm. Given a task,
Jev judges per idle, warm session whether its work so far covers the task:

```text
$ sessionkit peers "deploy commit 053cea4 together with ca0b16c"
Running sessions: status, context, cache read per call, cache left, fit.
  lmnl-56         idle    654K  $0.131/call  warm 55m  fit 0.80
  lmnl-2c         idle    235K  $0.047/call      cold
  lmnl-e8         busy    326K  $0.065/call  warm 59m

lmnl-56 fits. Send it the task with SendMessage to "lmnl-56"; it works in its own terminal, in parallel with you.
Every call it makes reads its whole context: $0.131 per call. For a long task, ask it for a briefing and give that to a subagent.
```

The skill tells the agent to run it before it spawns a subagent, so work goes to a session that
knows it instead of a fresh one that reads its way in. A warm session is not always cheaper:
every call reads its whole context, so for a long task a briefing from it plus a subagent costs
less. The fit line of 0.7 is not measured yet. The plugin's `agent.spawn` hook logs, in shadow,
which warm peers would have fit every subagent, in `~/.cache/sessionkit/agent-routes.jsonl`
under `warm_peers`. Jev costs about $0.0001 per session judged.

`~/.claude/sessions` is not a public contract of Claude Code; its format can change.

The status line, `systemMessage` and the block `reason` are shown to you only, so the checks
cost no tokens. One line does reach Claude's context: the note on what calls cost, above. Names
of other sessions in it are cut to letters, digits and `._-`, since any process of yours can
write the files they come from.

`sessionkit setup` writes the status line and the hooks into `~/.claude/settings.json`, with the path where
sessionkit is installed on your machine. A Claude Code plugin can ship hooks but not a status
line, which is why this is a command and not a plugin. It makes a backup first, leaves your
other settings and hooks alone, and can run again safely. It asks before it replaces a status
line that is not sessionkit's (`--force` skips the question). In zsh it also adds the hook for
`sessionkit next` to `~/.zshrc`, between two marker lines. `sessionkit setup --remove` takes
everything out again.

Set the limits with `SESSIONKIT_WARN_RESUME_ABOVE` (default 1) and `SESSIONKIT_BLOCK_COLD_ABOVE`
(default 2). Set the second very high to only warn in the status line.

## The architect

Coding agents solve the task in front of them well. Architecture degrades through many local
decisions that each look reasonable: an import across a boundary, a second copy of a concept, a
repository called from the wrong module. The next agent copies what it finds, so each of them
becomes precedent. sessionkit gives a repository an architect that stays out of the way until a
change touches the architecture.

- **Its memory** is `ARCHITECTURE.md` at the repository root: the system's shape, who owns what,
  the dependency rules, known debt with a policy per item, deliberate exceptions and open
  questions. Each statement carries its standing: `[confirmed]` by a person, `[observed]` in the
  code, or `[inferred 0.6]`. Decisions go in `docs/adr/`, one short file each. The memory
  describes the system as it is now; the architect rewrites it instead of appending to it.
- **The offer.** In a git repository without an architect, sessionkit asks once per session,
  after the first answer: *Onboard now*, *Not now* (asked again next session) or *Never for
  this project*, which is kept in `~/.config/sessionkit/architect.json`; delete the repository's
  line there to be asked again. The plugin option *Architect* set to off stops the question too.
- **Onboarding** sets the baseline: `/sessionkit:architecture onboard` in Claude Code. The
  architect surveys the repository, writes a draft and asks only about the assumptions whose
  wrong guess would mislead many later reviews ("Billing appears to own invoices. Correct?");
  an assumption that stays inside one module keeps its standing. Before you see them, Jev reads
  the code each recommended answer cites (`sessionkit architect rank`): questions where the code
  contradicts the draft come first, then what only intent can settle, and last what the code
  already shows. Your answers turn
  inferences into confirmed statements, and the current commit becomes the baseline. Debt found
  here is known debt: it gets no warnings until a change widens it.
- **While you work**, each plan the agent presents and each Edit or Write it makes goes to Jev,
  after it is done and without waiting. Jev sees the change and the memory side by side, so
  "checkout imports billing's repository" is judged against your rules, your debt and your
  exceptions. In the same request Jev holds the change against each decision record in
  `docs/adr/` on its own: does it follow it, go against it, or not touch it? A change that goes
  against a record is named in the note. Ordinary work ends there,
  at about $0.0005 per change plus a few hundredths of a cent per thousand words of decision
  records. For a change Jev finds architectural, the agent that made it gets one short note:

  ```text
  sessionkit architect (Jev): the change to src/checkout/mod.rs may affect the architecture in
  ARCHITECTURE.md: read as boundary (0.99); it may go against
  docs/adr/0002-billing-only-writes-invoices.md. Check it before you go on. …

  docs/adr/0002-billing-only-writes-invoices.md:
  # Only billing writes invoices
  …
  ```

  Each record it may go against comes with its text (up to 1500 characters), so the agent need
  not read it first. The agent checks a small case itself: one file against one named record. Otherwise it starts
  the architect in the background, a Sonnet subagent with the change, the memory and the code but
  not the agent's reasoning, so its view is its own. A broken rule or an architectural decision
  goes to you before the agent goes on; a decision you take, the architect writes into the
  memory. sessionkit starts no agent itself: in auto mode Claude Code's classifier refuses an
  agent a hook starts, so the agent the model starts is the one that works in every mode.
- **An audit** compares the repository with the baseline: `/sessionkit:architecture audit`
  reports what weakened, which dependencies and responsibilities moved, which statements in the
  memory are no longer true, and which debt grew or shrank. It proposes memory edits; you choose
  which to apply and whether the audit becomes the new baseline.

What it costs: one Jev question per change in an onboarded repository, nothing elsewhere. A note
comes once per file and reading in a session. Every check is logged in
`~/.cache/sessionkit/architect-checks.jsonl` with Jev's reading, noted or not.

Set it in `/config` under *Architect*: **advise** (the default), **shadow** (only the log; nothing
wakes) or **off**. `SESSIONKIT_ARCHITECT_WAKE` sets the line (default 0.6).
`SESSIONKIT_ARCHITECT_CLASSIFIER` names a program that answers instead of Jev: it gets
`{change, architecture, categories}` as JSON on stdin and prints `{"probabilities": {...}}`.

How well Jev separates, on twelve cases against a test memory (`python3
eval/architect/classify.py`): the seven architectural ones scored 0.91 to 1.00, among them a
change that widens known debt; four ordinary ones scored 0.00, among them a change inside known
debt; a new field on a type the memory calls a deliberate twin scored 0.53, under the line.
Twelve cases show the question works, not where the line belongs on your code; the log is there
to measure that.

Limits. The architect sees Edit, Write and plans, not files changed by a shell command (`cargo
add`, a code generator); an audit finds those later. Advice that arrives after the agent's last
request of a turn is read with your next message. A review is read-only by its tools, but it has
Bash for `git log`; an architect run from the skill may write only `ARCHITECTURE.md` and
`docs/adr/`. Without a TypeSafe key at hand, nothing is checked.

## sessionkit start

```sh
sessionkit "continue with the upload migration"         # Jev sees it continues one session
sessionkit "what was decided about the upload slots?"   # Jev sees a loose question and searches
sessionkit start --pick "continue"                      # choose one session from a list
sessionkit start --session "upload migration" "continue" # one session, by words from its title
sessionkit start --session 9d17b657 "continue"          # one session, by id or id prefix
sessionkit start --session last "continue"              # the newest session in this folder
sessionkit start --dry-run --verbose "…"                # show the context and scores only
sessionkit start --agent codex --pick "continue"        # start Codex instead of Claude Code
sessionkit start "…" -- --permission-mode plan          # everything after -- goes to the agent
```

A prompt without a command means `start`.

| Option | Meaning |
| --- | --- |
| `--agent <name>` | `claude`, `codex` or `jcode`. The default is `claude`, or `SESSIONKIT_AGENT`. |
| `--search` | Always search. Do not let Jev decide to continue one session. |
| `--pick` | Choose a session from a list of recent sessions in every project. Sessions from this folder come first. |
| `--session <s>` | Continue one session: an id, an id prefix, `last`, or words from its title. |
| `--all` | Search the sessions of every project, not only this folder. This is the default when the folder has no sessions. |
| `--top <n>` | Keep at most n sessions when searching (default 4). |
| `--yes` | Do not ask before spending. See "What start costs". |
| `--dry-run` | Print the context and the agent command. Do not start the agent. |
| `--verbose` | Print the Jev score of every session or turn. |

The context is written to `~/.cache/sessionkit/<time>.md`. Each agent gets it in its own way:

| Agent | How the context goes in |
| --- | --- |
| `claude` | `--append-system-prompt-file <context>`, then your prompt. |
| `codex` | `-c developer_instructions=<context>`, then your prompt. This replaces any `developer_instructions` in `~/.codex/config.toml` for this session. |
| `jcode` | jcode takes no first prompt. sessionkit waits for the new session to appear in `~/.jcode/active_pids` and sends the first message ("read the context file, then: your prompt") to that session id with `jcode transcript -S`. The message is also on the clipboard, in case it does not arrive. |

When you continue one session, the agent starts in that session's folder. When sessionkit
starts Claude Code, it notes the new session id and the sessions it came from in
`~/.cache/sessionkit/chains.jsonl`. A later run then offers the newer session instead of the
older one. sessionkit reads Claude Code sessions only; Codex and jcode sessions are not
searched.

### How start selects

Sessions are the JSONL transcripts in `~/.claude/projects`. An exchange is a prompt you typed
plus the last text Claude wrote before your next prompt. Shell commands you ran with `!` are
not exchanges.

**Deciding** (a prompt only):

1. Jev asks one yes/no question per session: did this session work on the subject of the new
   prompt? It sees a card with the title, folder, branch, every request with the start of its
   outcome, and the final outcome. Sessions at 0.3 or higher go on.
2. Jev chooses between the five best sessions and "new topic": does the prompt continue one
   of them? At 0.7 or higher for one session, sessionkit continues it. Between 0.3 and 0.7 it
   asks you. Otherwise it searches.

**Searching:** Jev asks one question per exchange in the best sessions: does this exchange
help with the new prompt? Exchanges at 0.5 or higher are kept, within about 6k tokens.

**Continuing one session** (chosen by Jev, or with `--session` or `--pick`):

1. Jev sees the whole session as an outline: every turn with its request, the start of its
   conclusion and the files it changed. The goal is your prompt plus the last three prompts
   of the session, so a vague prompt such as "continue" still works.
2. Jev asks per turn: does this still apply? A preference or rule you stated still applies
   after the work is done. A turn that a later turn replaced, or a one-off step, does not.
   It also asks per turn: does this request state a rule or preference?
3. Jev cuts every conclusion into paragraphs and list items, and asks per block: is this
   still open? Rules and open items need 0.7 or higher, because they go at the top. At most
   eight open items are shown, the strongest ones.
4. Git adds what changed since the session: the branch now, the commits after it, the
   uncommitted changes, and which files of the session changed afterwards.
5. The context starts with what changed since, your rules, and the open items. Then come the
   files the session changed, its last commands, the last compaction summary, and the turns:
   the first, the last two, and the ones that still apply. The budget is about 15k tokens.

### What start costs

Before it sends anything, `start` runs the whole selection dry: no network, a neutral answer
to every question, and the largest sessions wherever the real run would pick by score. It
counts the characters it would send and prints an upper bound:

```text
sessionkit: at most $0.04 (32 requests, at most 974,524 tokens)
sessionkit: 149 sessions read, 4 relevant (209,834 Jev tokens, about $0.0088)
```

Above $0.05 it asks before it spends anything. Set the limit with `SESSIONKIT_CONFIRM_ABOVE`,
or skip the question with `--yes`. Measured on 27 September 2026:

| Run | Estimate | Real |
| --- | --- | --- |
| Continue one session | at most $0.0017 | $0.0017 |
| Search 149 sessions, loose question | at most $0.04 | $0.0088 |
| Search 149 sessions, continuation | at most $0.04 | $0.0060 |

The estimate converts characters to tokens at 3.5 per token; a real run measured 3.76, so the
estimate stays on the high side.

### The TypeSafe key

All Jev commands use the same TypeSafe key. On macOS, configure it explicitly:

```sh
sessionkit auth login     # hidden prompt; stores your key in the macOS Keychain
sessionkit auth status    # shows the source, never the key; no API request
sessionkit auth logout    # removes stored keys, including the legacy jev-start entry
```

`login` stores the key without validating it with TypeSafe or making a paid request.
For automation, `sessionkit auth login --stdin` reads the key from standard input; do not
pass secrets as command arguments. On other platforms, use `TYPESAFE_API_KEY`.

The lookup order is:

1. `TYPESAFE_API_KEY` in the environment (not automatically persisted).
2. The macOS Keychain, service `sessionkit`, account `TYPESAFE_API_KEY` (or legacy `jev-start`).
3. Optional 1Password CLI fallback, **only** if `SESSIONKIT_OP_REFERENCE` is set:

   ```sh
   export SESSIONKIT_OP_REFERENCE="op://your-vault/your-item/credential"
   ```

There is no default vault or item. On macOS, a key read from 1Password is cached in Keychain.
Hooks never invoke 1Password. If the shared API client receives a 401 for a Keychain key,
its refresh uses 1Password only when explicitly configured; otherwise it asks you to run
`sessionkit auth login` again. `grep` reports authentication failure without automatic refresh.
`logout` does not unset environment variables or delete items from 1Password.

### What leaves your machine

`usage` sends nothing. `start` and `measure` send parts of your transcripts to TypeSafe:
session cards, outlines and exchanges. In a repository onboarded for the architect, every plan
and every diff the agent writes goes to TypeSafe together with `ARCHITECTURE.md`, and onboarding
sends the code its calibration questions cite. The kept exchanges also go into the context file and
the new session. sessionkit does not redact anything, so a secret that is in a transcript can
end up in all of these places.

## sessionkit grep

```sh
sessionkit grep "How are telemetry events recorded and sent?"      # in this folder
sessionkit grep "Which tests cover retry behavior?" ./src          # in a subfolder
sessionkit grep "Where is the quote price calculated?" --max-source-bytes 20000
```

Use it when you know the behavior you need but not where it lives. It walks the repository
breadth first and asks Jev per folder whether it is worth exploring and per file whether it
helps; then, per declaration in the files it admits, whether that block implements or tests
what the question asks. The output starts with the relevant files and their roles
(implementation, caller, test, fixture, helper), then verbatim source with line numbers, then
the declarations as `name@start-end`. It is evidence for the agent, not a generated answer.

`sessionkit setup` installs the skill `~/.claude/skills/sessionkit/`, so Claude Code knows when
and how to use `grep` and `ask`.

With function hooks enabled, the plugin also exposes **`mcp__sessionkit__search_code`**
and **`mcp__sessionkit__ask_file`** as ordinary agent tools over these same CLI commands.
They are visible without a ToolSearch lookup. A short navigation rule goes in the main
prompt when both tools are offered, and in subagent tasks: unknown behavior location →
search_code; exact symbol/literal → Grep; known file, specific facts → ask_file. Search
returns at most 12,000 bytes of source by default (adjustable up to 100,000); ask_file
batches up to ten questions or finds lines for one question. Nothing automatically
converts a Read/Grep into a paid request. The tools send eligible source to TypeSafe
when called, with the CLI's normal exclusions and credentials; no option to include
sensitive files is exposed. Disable *Code search tools* (`codeSearchTools`) in plugin
options for CLI-only use.

Live-tested on Claude Code 2.1.291: a main agent used search_code, a general-purpose
subagent used ask_file, and an Explore subagent chose search_code for a behavior search
without a tool name in its delegated task. This verifies access and one small adoption
case, not a token or latency benchmark. See [verification notes](code-search-tools.md).

- **What leaves your machine.** Eligible source goes to TypeSafe. Hidden files, dependency and
  build folders, files that `.gitignore` or `.ignore` exclude, sensitive names (`.env`, keys)
  and private keys in content stay out; `--hidden`, `--no-ignore`, `--include-dependencies`
  and `--include-sensitive` each bring one category back.
- **What it costs.** From about ten Jev calls for a small repository to a few hundred for a
  large one. Answers are cached in `~/.cache/sessionkit/grep` for seven days, so the same
  question on unchanged files is free; `--no-cache` skips the cache.
- **Output.** All output goes to stdout. Exit codes: 0 complete, 2 incomplete (some parts could
  not be judged; the output says which), 1 failed, 130 interrupted.

### Compact agent presentation and PHP support

`sessionkit grep --agent-view --max-files 8 --max-source-bytes 12000 "question"`
uses the same discovery pipeline with a bounded shortlist. It reports the total
result count, preserves failure/incomplete diagnostics, and reserves source for
multiple files before expanding the highest-ranked excerpts. Oversized snippets
are cropped near high-scoring declarations and explicitly labeled partial.
Increase `--max-files` (1..100) or narrow the question/root to inspect more.
With no source budget specified, agent-view uses 12,000 bytes; the ordinary CLI
view remains full, with its existing unlimited source default.

PHP and PHTML files now use tree-sitter declaration boundaries. Methods are
separate selection units rather than whole class bodies; class headers remain
available as context. Malformed or oversized source falls back to text handling.
The parser identity changed, so old cached decisions are not reused.

### How it relates to jevgrep

The Python/JavaScript/TypeScript port originally asked Jev the same questions as `jg` 0.4.2 (model `jev-1.13.0`): the same
wording, thresholds, batch limits, key order and traversal. Compared with `jg` against a local
stand-in for TypeSafe, on eleven questions in eight repositories, it sent the same requests byte
for byte and printed the same output. The one difference in requests is the order of the
evidence list in the second pass, which follows which file finished first; that order varies
between runs of `jg` too. It is about four times faster, and needs no Node.js.

jevgrep parses Python with CPython 3.11 and TypeScript with the TypeScript compiler. Here
Python goes through tree-sitter and TypeScript and JavaScript through oxc. On 1,500 Python files
and 1,494 TypeScript and JavaScript files the declaration boundaries and comments came out the
same as jevgrep's, except for one Python file where tree-sitter misreads a line continued inside
parentheses with less indentation. What CPython 3.11 rejects falls back to text, as in jevgrep:
Python 2, 3.12 type parameters and aliases, 3.12 f-strings, 3.14 t-strings, inconsistent
indentation or tabs, empty blocks, and parameters or arguments in the wrong order. A few errors
that only CPython's compiler reports (such as `del 1`) still parse here.

Other differences: the cache is its own (answers computed on jevgrep's parsers are never
reused), the key is sessionkit's TypeSafe key, and a refused key says so instead of "Command
failed". jevgrep is MIT-licensed; see [licenses/jevgrep-MIT.txt](licenses/jevgrep-MIT.txt).

## sessionkit ask

```sh
sessionkit ask src/upload.ts "Does this file retry a failed upload?" \
  "Which HTTP client does it use?" --options fetch,axios,got \
  "Does it talk to a real database?" --no "Mocks and fixtures do not count."
sessionkit ask --filter "Does this file make a network call to an external service?" src/
sessionkit ask --find src/config.rs "Where is the request timeout set?"
```

```text
0.89  Does this file write to ~/.zshrc?
      line 276  std::fs::write(path, &next_zshrc).map_err(|e| e.to_string())?;
1.00  serde_json  (confidence 1.00)  Which serialization library does it use?
      line 20  use serde_json::{Map, Value, json};
0.03  Does it talk to a database?

src/setup.rs · 13107 chars · 3866 tokens · $0.0002 · jev-1.13.0
```

Use it when an agent needs a fact about a file rather than its contents. The file goes to Jev
and a probability comes back: above 0.70 read it as yes, below 0.30 as no, in between read the
file. Up to ten questions about one file cost the same as one. Under every answer of 0.30 or
more stands the line that answers it, found as `--find` finds it, so an open question ("where
are the files of this client?") gets its answer and a yes can be checked. That costs one more
request per 200 non-empty lines, for all questions together; files beyond 2,400 non-empty lines
get no line for what lies further on.

- **Questions.** A plain question is yes/no. `--options a,b,c` after a question makes it a
  choice (`none of these` is added); `--yes` and `--no` say what counts, with the boundary cases.
- **`--filter`** asks one yes/no question about every file, highest first. A folder is searched
  with the rules of `grep`: no hidden, ignored or dependency files, at most 200 files
  (`--max-files`).
- **`--find`** returns the likeliest lines and first says whether the file answers the question
  at all: "Not in this file" below 0.30, "Partly answered" below 0.70.
- **What leaves your machine.** The file, cut into parts of 60,000 characters with 20 lines of
  overlap, at most eight parts. A file with a sensitive name or a private key in it stays here
  unless you pass `--include-sensitive`; binary files are never sent.

Compared with the ask-jev plugin it asks the same questions in the same shape, runs from the
command line instead of an MCP server, needs no Node.js, uses sessionkit's key, keeps sensitive
files back, searches folders for `--filter`, and prints the cost.

## sessionkit compact

Claude Code compacts a conversation by asking a model to summarize it. A summary is lossy: a
path, an exact error or a constraint can disappear although it matters later. `sessionkit
compact` rewrites nothing. It shows Jev the whole conversation, with every tool output replaced
by a short note, and asks per tool call two questions: should the call stay, and should its
full output stay verbatim. An output it lets go is cut to its first 300 characters and a note;
a call it lets go is dropped together with its output. The first message and the newest six
are never touched, and all text of you and Claude stays as it was. The method and the limits are
those of fast-jev-compaction 0.3.0; the question about the output is measured and reworded (below).

`sessionkit setup` installs it as a Claude Code plugin with one function hook on
`session.compact`: `/compact`, auto-compaction and Claude Code's precomputed compaction all go
through `sessionkit compact --hook`. When that fails, or frees less than a quarter of the
characters, Claude Code writes its own summary as before. A toast says which happened.
After a `/compact` that sessionkit did, the agent says so itself: a short prompt asks it to tell
you, in the language of the conversation, how many tokens the conversation went from and to (as
Claude Code measured them), and to propose how to go on. That costs one model call; the cache
write it makes, your next message would make anyway. After an auto-compaction the agent just
goes on with its work.

**Fold a turn when it ends** is an option, off by default: turn on *Fold a turn when it ends*
in `/config`. A tool output is read once and then rides along in every later call. When a turn
ends that added 20,000 characters of tool output or more, the plugin starts a compaction of its
own and answers it with `sessionkit compact --fold`: every earlier message stays as Claude Code
has it, and in the finished turn the output of each call that changed no file is cut to its
first 300 characters. Prompts, answers, thinking and every call with its input stay. No model is
asked, Jev neither. The cut is at the end of the conversation, so everything before it stays in
the prompt cache. Measured with Claude Code 2.1.287 and Haiku: a turn that read a file of 22K
tokens ended at 74.5K tokens of context; the first call of the next turn read 51.6K from the
cache and wrote 0.8K. Interactive sessions only: a headless session (`claude -p`, the SDK)
cannot start a compaction between turns.

```sh
sessionkit compact last               # what a compaction now would keep; changes nothing
sessionkit compact 9d17b657 --verbose # per tool call: the decision and both probabilities
sessionkit compact --fold                         # for the plugin: fold the last turn of the messages on stdin
sessionkit compact --measure build cuts.json      # labelled cuts from every session (free)
sessionkit compact --measure run cuts.json train  # measure the questions on them (costs money)
```

Function hooks are an early-access part of Claude Code: they need
`CLAUDE_CODE_ENABLE_FUNCTION_HOOKS=1`, which `setup` puts in the settings, and their interface
can change between releases. Tested with Claude Code 2.1.284: a `/compact` on a session of
89,812 tokens left 14,770 in 385 ms, with the conversation verbatim and three of four file
reads gone. If fast-jev-compaction is installed too, turn one of them off; `setup` says so.

**Measured, and the output question reworded.** `--measure build` cuts real conversations at
points between 5% and 95% of every stretch between two compactions, as auto-compaction would,
and labels each Read that compaction may touch by what happened to the file after the cut:
*came back* (edited, read again, or named in another tool's input, such as a grep) or *unused*
(never again). Screenshots and the output of background tasks are left out, and cuts are split
into train and test by session. `--measure run` compacts every cut and reports, within each cut,
how often an output that came back scores above one that did not (0.5 is chance). Run of 29
September 2026: 140 sessions, at most two cuts each, 108 train cuts and 80 test cuts:

| Output question | Train | Test |
| --- | --- | --- |
| fast-jev-compaction: "…re-running the tool would not do" | 0.56 – 0.60 | 0.56 |
| Now: names the input, asks whether the work is still going on | 0.67 | 0.70 |
| A rule without Jev: the more recent, the more likely | 0.58 | 0.56 |

On test the new wording gained 0.14 per cut (95% interval 0.02 to 0.26). Two identical runs
differ by up to 0.05, because most answers lie close together. fast-jev's wording put almost
every output below 0.3, so at the threshold of 0.5 nothing was kept; the new one answers around
0.7, and at its threshold of 0.78 it kept 40% of the outputs that came back and 10% of those that
did not (test). The call question is fast-jev's: a rewording helped on train and hurt on test.
A list of the newest calls in the state did not help, and neither did mixing in recency.

What stops it: most wrong orders are two files of one task, of which one happened to come back.
That cannot be told from the conversation, so this label has a ceiling not far above 0.70. A
better measurement would count the cost: tokens freed against tokens read again.

## sessionkit measure

`measure` checks the selection of `start` with labels that code makes, so a run needs no hand
work. Results go to `~/.cache/sessionkit/measure/` and hold ids and scores, not text.

**A run costs money.** It sends parts of your transcripts to TypeSafe: about $0.20 for the
default size (30 cuts, 20 routing pairs), about $0.06 for a smoke test with
`--cuts 3 --routes 2`. It first estimates the cost dry and asks before it spends. `--estimate`
stops after the estimate; `--yes` skips the question.

- **Turns.** It cuts a session at turn k and uses the real next prompt. It plants five turns
  with a known label in the free places: the real turn after the prompt (applies), the first
  turn of a session in another project (does not), a stated rule, a plain task, and one
  conclusion with an open block and a done block.
- **Routing.** A later prompt of a session must route to the cut session, which sits among
  150 others. The first prompt of a session, with that session removed, must route to a new
  topic.

Run of 27 September 2026 (seed 7, 8 cuts, 24 routing items, $0.21):

| Question | Right plant above wrong one | Right plant passes | Wrong plant passes | Real items that pass |
| --- | --- | --- | --- | --- |
| Turn still applies (0.5) | 7 of 8 | 75% (median 0.73) | 0% (median 0.25) | 67% (median 0.60) |
| Rule or preference (0.7) | 8 of 8 | 100% (median 0.95) | 0% (median 0.06) | 8% |
| Block still open (0.7) | 6 of 6 | 83% | 0% | 11% |

- A prompt from another project lowers the real turn scores by 0.15 (median), so the turn
  question reads the prompt.
- "Still applies" ranks well but keeps two thirds of real turns at 0.5. The budget, which
  takes the strongest turns first, does the real selection.
- 11% of real blocks read as open: about fifteen per session, more than a reader wants. Hence
  the cap of eight.
- Routing: `start` continued on its own four times, three times right (0.72 to 0.87) and once
  wrong (0.67). The threshold is now 0.7. Of the other later prompts, the right session was
  often not among the candidates: a mid-session prompt such as "can you also add X" does not
  name its subject. A real prompt usually does, so these items are harder than real use.
- First prompts routed to a new topic 9 times out of 12.

Earlier labels from shared files and shared terms were dropped: most turns change no file,
and shared terms scored at chance.

## Limits

- The measurement above is small: 8 cuts and 24 routing items from one person's sessions.
- `sessionkit compact` depends on function hooks, an early-access part of Claude Code, and its
  questions do not yet separate needed from unneeded tool output (see above).
- After a compaction by `sessionkit compact`, the transcript holds the kept messages twice:
  before the boundary and after it. `start` and `usage` skip the copies.
- Jev sees later turns only in short form, so an open item that a later turn finished can
  still show as open.
- Session chains only cover Claude Code sessions that sessionkit started. A session you
  continued by hand, or with Codex or jcode, still competes with the session it came from.
- Jev performs best in English. Prompts and transcripts in other languages work, but have not
  been evaluated.
- `--session last` picks the newest session in the folder. If a session is running there, it
  picks that one.
- The cold-cache count in `usage` is a heuristic; a changed prompt prefix also rewrites the
  cache without a pause.
- The transcript format of Claude Code is not a public contract and can change.
- A fold (the option *Fold a turn when it ends*) is a compaction to Claude Code: the screen shows
  "Conversation compacted", the PreCompact and SessionStart hooks of your settings run, and the
  transcript file gets one more copy of the conversation at every fold. A session resumed after a
  fold writes the conversation to the cache again.

## Environment

| Variable | Meaning |
| --- | --- |
| `TYPESAFE_API_KEY` | Your TypeSafe key for Jev; overrides Keychain and optional 1Password. |
| `SESSIONKIT_OP_REFERENCE` | Optional 1Password secret reference (`op://vault/item/field`); no default. |
| `SESSIONKIT_AGENT` | The default agent for `start`. |
| `SESSIONKIT_CONFIRM_ABOVE` | Ask before spending more than this many dollars (default 0.05). |
| `SESSIONKIT_MODEL` | The Jev model (default `jev-latest`). |
| `SESSIONKIT_WARN_RESUME_ABOVE` | Warn on a resume whose cache write costs more than this (default 1). |
| `SESSIONKIT_BLOCK_COLD_ABOVE` | Stop a message to a cold session once above this cost (default 2). |
| `SESSIONKIT_ARCHITECT_WAKE` | Wake the architect at this relevance or above (default 0.6). |
| `SESSIONKIT_ARCHITECT_CLASSIFIER` | A program that classifies changes instead of Jev; `jev` or empty for Jev. |

The `JEV_START_*` names from before the rename still work.
