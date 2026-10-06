<!-- sessionkit architect: baseline pending after public history reset -->
# Architecture

SessionKit is a Rust command-line application with an embedded Claude Code
plugin. Claude Code is the primary integration; Codex and jcode currently have
context-launch support rather than equivalent plugin integrations.

This document describes implementation boundaries and maintenance constraints.
Architectural decisions are recorded separately in [docs/adr/](docs/adr/).
The public history was reset during documentation sanitization. A new audit
baseline must be established against the current public history before using
commit-based architecture comparisons.

## System overview

```text
Claude Code
  ├── Plugin function hooks and agent tools
  │     └── SessionKit CLI
  └── Settings hooks and status line
        └── SessionKit CLI

SessionKit CLI
  ├── Local transcript and usage analysis
  ├── Context selection and compaction
  ├── Code search and file questions
  ├── Architecture-change classification
  ├── Credential and filesystem access
  └── TypeSafe / Jev requests where required
```

The application has one binary entry point, `src/main.rs`, and no separate
library crate. Plugin modules run inside Claude Code's function-hook environment
and access host services through the mods API. Paid classification and search
requests are implemented in Rust, not duplicated in JavaScript.

## Component responsibilities

| Component | Responsibility |
| --- | --- |
| `src/main.rs` | Command dispatch and top-level error handling. |
| `src/setup.rs` | Install settings hooks, status line, skills and embedded plugin files. |
| `src/transcript.rs` | Transcript entry representation. Transcript discovery remains command-specific. |
| `src/usage.rs`, `src/pricing.rs`, `src/cache.rs` | Local usage accounting, model pricing and cache-state calculations. |
| `src/start.rs` | Select relevant prior context and launch an agent. |
| `src/next.rs`, `src/peers.rs` | Fresh-session handoffs and discovery of relevant running sessions. |
| `src/compact.rs`, `src/trim.rs` | Verbatim context compaction and tool-output analysis. |
| `src/ask.rs`, `src/grep/` | File questions and behavior-oriented code search. |
| `src/jev.rs`, `src/auth.rs` | Shared Jev request handling and credential lookup. Semantic search owns a separate evaluator. |
| `src/architect.rs` | Classify changes against architecture memory and decision records. |
| `src/skills.rs`, `src/measure.rs` | Workflow analysis and context-selection evaluation. |
| `src/awareness.rs`, `src/js.rs` | Cost-awareness state and JSON-access utilities. |

## Claude Code plugin

`plugin/hooks/index.js` registers the runtime modules. The plugin contains:

- Search tools over the existing CLI: `search_code` and `ask_file`, with bounded
  inputs, normal source exclusions and navigation guidance.
- Context and cache integrations: compaction, turn folding, cold-cache dialogs,
  large-file outlines and eligible unread-file edit retries.
- Agent integrations: shadow routing, cost-awareness notes and repeated-report
  reduction.
- Architect integrations: change notifications, an architect agent definition,
  and skills for onboarding, auditing and recording decisions.
- Experimental Auto-denial approval and diagnostic permission logging.
- An optional output style and plugin configuration manifests.

Plugin files are embedded at compile time by `src/setup.rs`. Every runtime file
imported by the plugin must be included in the installer's file inventory; the
setup tests check that inventory. Adding a source file alone does not update an
already installed plugin or binary.

## Data and control flows

### Code navigation

The agent chooses a search or file-question tool. The plugin validates inputs
and invokes the CLI through `$.process.run`, without a shell. Rust applies source
eligibility rules and sends eligible data to TypeSafe. The plugin returns source
evidence and diagnostics rather than synthesizing an answer. Exact-symbol
searches remain the responsibility of ordinary Grep or equivalent tools.

### Context management

Local transcript readers supply usage analysis and context selection. Jev
selects retained context; it does not rewrite the retained conversation.
Compaction may drop tool calls or shorten their output while preserving retained
conversation text verbatim. Claude Code can fall back to its normal summary
compaction when the SessionKit path fails or does not free sufficient context.

### Architect review

An onboarded target repository stores its memory in `ARCHITECTURE.md` and
`docs/adr/`. Hooks send eligible plans and edits to the Rust classifier. A
relevant result adds a note to the coding agent's context; the coding agent
reviews the case or starts the architect through its normal agent workflow.
Architect advice does not veto changes. The user approves architectural decisions
and memory updates. Hooks do not directly spawn architect review agents.

### Experimental permission handling

The initial permission check remains unchanged. The approval integration
correlates `classic.PermissionDenied` with the active call and checked inputs,
then requests explicit approval for one retry. Approval is call-scoped,
argument-specific and never persisted as a permission rule. Explicit rules,
hook decisions and organization ceilings remain in force. This path is disabled
by default and is not yet live-verified after a real classifier denial.

## Dependency and ownership rules

- Plugin JavaScript reaches Jev-backed logic through the SessionKit CLI.
- New Jev-backed Rust commands use the shared client in `src/jev.rs`, unless a
  separately documented protocol or policy requires an exception.
- `src/auth.rs` owns credential lookup; `src/pricing.rs` owns the built-in price table.
- Transcript representation and command-specific transcript discovery remain
  distinct responsibilities.
- Local analysis must not acquire an unnecessary network or authentication dependency.
- Preserve verbatim retained context and make incomplete search results explicit.
- Keep architecture advice separate from permission enforcement.
- Avoid introducing additional platform-specific coupling into portable logic.

## Storage and external services

There is no application database or long-running SessionKit server. State uses
local files and the host's credential facilities:

| Location | Purpose |
| --- | --- |
| `~/.claude/` | Claude Code transcripts and installed settings. |
| `~/.cache/sessionkit/` | Selected context, search caches, handoffs, logs and evaluation results. |
| `~/.config/sessionkit/` | Persistent feature preferences such as declined architect onboarding. |
| macOS Keychain | Stored TypeSafe credentials. |
| Target repository | Architecture memory and decision records. |

`usage` and status-line calculations are local. Jev-backed workflows send source,
conversation data or architecture changes to TypeSafe as required. Transcripts
are not automatically redacted. Default source exclusions reduce accidental
exposure but are not a comprehensive secret-detection guarantee.

## Accepted design decisions

- [ADR 0001](docs/adr/0001-grep-keeps-its-own-jev-client.md): semantic search keeps its own evaluator and Jev client.
- [ADR 0002](docs/adr/0002-no-policy-module-for-cold-and-large-sessions.md): settings and function-hook policies remain separate.
- [ADR 0003](docs/adr/0003-transcript-lookups-stay-separate.md): transcript discovery remains command-specific.
- [ADR 0004](docs/adr/0004-prices-are-a-built-in-table.md): model prices are a built-in table.
- [ADR 0005](docs/adr/0005-the-architect-is-a-plugin-agent-woken-by-jev.md): the coding agent initiates architect reviews after hook notifications.

## Maintenance constraints

- **AD-001 — Module cycles.** Existing coupling includes `claude`/`architect`,
  `claude`/`peers` and `start`/`next`. Review changes that expand these dependency
  cycles; prefer narrower interfaces for new responsibilities.
- **AD-002 — Broad coordinator modules.** `claude.rs` and `start.rs` combine several
  responsibilities. Avoid adding unrelated responsibilities. A future extraction
  of session reading from launch orchestration should preserve command-specific
  discovery policies. No refactoring deadline is implied.
- **AD-003 — Shared filesystem helper ownership.** General-purpose callers obtain
  some path helpers through `usage`. Avoid propagating that ownership pattern
  into new modules; centralize it when a focused refactoring is justified.

Settings hooks and function hooks intentionally overlap in some cold-cache and
large-context policies. Semantic search's separate client and multiple transcript
lookups are documented exceptions, not accidental duplication to consolidate
without reviewing their distinct behavior.

## Build, verification and compatibility

- `cargo test --locked` validates Rust behavior and the plugin file inventory.
- `claude plugin test plugin` validates function hooks and tool wrappers.
- `eval/` contains optional evaluation scripts outside the runtime dependency graph.
- Release automation builds macOS archives. Legacy private Homebrew publication
  is restricted to its original repository; the public installation path is
  documented in the README.
- macOS is the tested installation path. Keychain and terminal handoff behavior
  require platform-specific facilities.
- Claude Code function hooks, transcript formats and running-session metadata may
  change between versions. Live verification complements mocked tests; neither
  small fixture tests nor local measurements establish general performance guarantees.
