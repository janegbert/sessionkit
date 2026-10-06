# SessionKit

**Find code by behavior. Keep context lean. See what Claude Code costs.**

Coding agents spend a lot of context finding their way around: searching for the
wrong keywords, reading entire files for one fact, and carrying old tool output
through every later call. SessionKit gives Claude Code tools to do less of that.

It combines semantic code search, focused file questions, architecture review,
context management and local usage analysis in one Rust CLI and Claude Code plugin.

## What you get

| | |
| --- | --- |
| **Search by behavior** | Find implementation, callers and tests with a question—not a guessed symbol name. |
| **Ask instead of reading everything** | Get facts or relevant lines from a known file; batch several questions in one call. |
| **Keep useful context, not a summary** | Select relevant history for a fresh session, or compact tool calls and output while retaining your conversation verbatim. |
| **Understand your usage** | See which sessions, models and tools consume tokens, plus cache costs inside Claude Code. |
| **Keep architecture intentional** | Give your project an architect with persistent memory, decision records and reviews when changes touch its rules. |

Search and context selection use [Jev from TypeSafe](https://docs.typesafe.ai).
Usage analysis runs locally and needs no API key.

## Get started

**Claude Code is the primary integration.** macOS is the tested installation
path; plugin features use Claude Code's early-access function hooks. The new
agent tools have been live-tested with Claude Code 2.1.291.

Build from source with a current Rust toolchain:

```sh
git clone https://github.com/janegbert/sessionkit.git
cd sessionkit
cargo install --path . --locked

sessionkit setup       # install the Claude Code plugin, hooks and status line
sessionkit auth login  # store your TypeSafe key in the macOS Keychain
```

No TypeSafe key yet? Start with `sessionkit usage`; it is entirely local.
For automation or other platforms, set `TYPESAFE_API_KEY` instead of using Keychain.

Setup backs up your Claude Code settings and preserves unrelated entries.
Restart Claude Code after setup. `sessionkit setup --remove` removes the integration.

## Find the code you mean

```sh
sessionkit grep "Where are failed uploads retried?" ./src
sessionkit grep "Which tests cover permission denials?" --max-source-bytes 12000
```

Results include ranked files, their roles and **verbatim source excerpts with
line numbers**. They are evidence to inspect, not a generated explanation.
Use ordinary `rg` when you already know an exact symbol or string.

The plugin exposes this same search as **`mcp__sessionkit__search_code`**, alongside
**`mcp__sessionkit__ask_file`** for focused file questions. Both are directly visible
to agents, with a short navigation rule for the main agent and subagents. An
Explore subagent chose search_code in a live fixture test without a prescribed
tool name. Broader adoption and token savings still need measurement.

The agent search defaults to an eight-file shortlist and shares its 12 KB source
budget across the selected files. Partial snippets are labeled; increase
`max_files` to reveal additional results. PHP methods, Python declarations and
JavaScript/TypeScript declarations are parsed for focused source selection.
The CLI keeps its full result view; use `--agent-view` for the compact presentation.

Disable *Code search tools* in the plugin options if you prefer CLI-only use.
[Tool design and verification →](docs/code-search-tools.md)

## Ask a file, not your entire context

```sh
sessionkit ask src/upload.ts \
  "Does it retry failed uploads?" \
  "Does it log a final failure?"

sessionkit ask --find src/config.rs "Where is the timeout set?"
```

Answers include probabilities and supporting lines. Ask up to ten questions
about one file together, then read only the context you still need. Uncertain
answers and incomplete searches are reasons to inspect the source—not proof
that something is absent.

## Pick up the work without dragging everything along

```sh
sessionkit start --session last "Continue the upload retry fix"
sessionkit start --dry-run --session last "Continue the upload retry fix"
```

Start a fresh Claude Code session with selected context from an earlier one.
Kept text stays verbatim. The plugin also integrates with `/compact`, warns
before expensive cold-cache rebuilds and shows context/cache costs in the status
line. [How context selection and compaction work →](docs/reference.md#sessionkit-start)

## See where the tokens go

```sh
sessionkit usage --days 7
sessionkit usage --tools
sessionkit usage --json
```

Analyze local Claude Code transcripts by session, model, project and tool.
Reported dollar amounts are API-equivalent estimates, **not your subscription
bill**. [Usage details →](docs/reference.md#sessionkit-usage)

## Give your project an architect

In Claude Code, from your project's directory:

```text
/sessionkit:architecture onboard
```

The architect surveys the repository, drafts **`ARCHITECTURE.md`** and asks you to
confirm the assumptions that matter. Decisions are recorded in **`docs/adr/`**.
Once onboarded, Jev checks plans and edits against that memory; potentially
architectural changes prompt the coding agent to review the issue or consult the
architect. You decide which changes and decisions to accept—it is advice, not
an automatic veto.

Use `/sessionkit:architecture audit` to review drift since the baseline, or
`/sessionkit:architecture record <decision>` to record a decision explicitly.
The *Architect* option defaults to **advise**; **shadow** logs checks without
notifying the agent, and **off** disables checks and the onboarding offer.
Checks require a TypeSafe key and send changes and architecture memory to TypeSafe.
[Architect details and limitations →](docs/reference.md#the-architect)

## Keep project workflow knowledge compact (experimental)

Enable *Compact project workflow* to coach agents to improve their approach within
the task, without automatically turning observations into skills or memories.
Use `/sessionkit:project-workflow onboard` to propose one compact project skill,
`review` to consolidate it, or `propose <lesson>` to assess a recurring procedure.
Existing docs stay authoritative; references load only when needed. Persistent
changes require an exact proposed diff and explicit approval. This is instruction
coaching, not a blocker for native memory writes. [Design and limits →](docs/project-workflow.md)

The proposed as-built **surveyor** is still a research prototype, not a replacement
for onboarding. Its [method and evaluation](docs/survey-method.md) separate source
observations from intent and check representative paths and counterevidence. The
first comparison does not yet justify promotion.

## Configure the plugin

In Claude Code, open **`/config`** and find SessionKit's plugin options. You can
also inspect and change them from your terminal:

```sh
claude plugin configure sessionkit@sessionkit

# Example: enable outlines and the experimental cost notes.
printf '%s\n' '{"readOutline":"true","costAwareness":"true"}' |
  claude plugin configure sessionkit@sessionkit --values-stdin
```

The CLI expects string values, including `"true"` and `"false"`. Omitted options
keep their current values. Restart Claude Code after changing plugin configuration.

| Option / config key | Default | What it changes |
| --- | --- | --- |
| Code search tools / `codeSearchTools` | On | Offers search_code and ask_file with navigation guidance. |
| Compact project workflow / `projectWorkflow` | Off | Experimental coaching and explicit project-skill onboarding/review; no automatic persistence. |
| Architect / `architect` | `advise` | Choose `advise`, `shadow` or `off`; checks begin after onboarding. |
| Outline first / `readOutline` | Off | First whole read of a large file shows its opening and declarations; a second read returns it whole. |
| Fold a turn / `foldTurns` | Off | Cuts large read-only tool outputs after a turn; interactive sessions only. |
| Lower effort / `applyEffort` | Off | Applies Jev's lower effort recommendation; on unverified models/providers it can cause cache rewrites. |
| Cost awareness / `costAwareness` | Off | Experimental context-cost notes for the agent and subagents. |
| Ask after an Auto denial / `askOnAutoDenial` | Off | Experimental, explicit one-time approval; real classifier-denial retry is not yet live-verified. |
| Permission probe / `permissionProbe` | Off | Diagnostics only; logs potentially sensitive arguments without changing decisions. |

Try optional features one at a time rather than enabling everything. For folding
and effort-routing behavior, see the [full reference](docs/reference.md); for the
Auto-denial experiment, read its [verification limits](docs/auto-permission-probe.md)
before enabling it. Disable any boolean option by setting its string value to
`"false"`, or disable architect checks with `"architect":"off"`.

## Privacy, cost and current limits

- **Local analysis stays local.** `usage` makes no network requests.
- **Jev features send data to TypeSafe and incur API costs.** Search/file questions
  send eligible source; context selection and compaction send conversation data.
  Architect checks, when enabled in an onboarded repository, send changes and
  architecture memory. Transcripts are not automatically redacted.
- **Code search excludes sensitive names/private keys, hidden, ignored and
  dependency files by default.** This is not a guarantee that every secret is
  detected. The agent tools expose no flag to bypass these exclusions.
- **Claude Code internals can change.** Function hooks and transcript formats
  are not stable contracts. Review experimental plugin options before enabling them.
- **Codex support comes later.** `start` can already launch Codex or jcode with
  selected Claude Code history, but they do not have the full plugin integration.

The optional [ask-after-Auto-denial route](docs/auto-permission-probe.md) is
experimental and off by default. Its retry after a real classifier denial has
not yet been verified live.

## More

- [Full reference: commands, setup, architecture, costs and configuration](docs/reference.md)
- [Agent search tools](docs/code-search-tools.md)
- [Compact project workflow experiment](docs/project-workflow.md)
- [Architecture overview](ARCHITECTURE.md)
- [Changelog](CHANGELOG.md)

For contributors:

```sh
cargo test
claude plugin test plugin
```

Code-search and compaction implementations build on
[jevgrep](https://github.com/dzhng/jevgrep), the ask-jev plugin and
[fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction).
Their license notices are preserved in [licenses/](licenses/).
