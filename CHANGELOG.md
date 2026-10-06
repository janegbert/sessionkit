# Changelog

Notable product changes to SessionKit. Entries describe functionality and
compatibility changes; they do not include development conversations or private
usage data. Versions before the public repository are retained as historical
release notes, not links to public release artifacts. The project was named
`jev-start` before version 0.3.0.

## Unreleased

- Add a controlled survey evaluation with/without only native SessionKit search
  tools, synthetic-root CLI guards and conservative Jev cost journals. Real
  dispatch works in a separate forced smoke; unforced runs do not choose the
  tools, so no adoption, savings or onboarding promotion is claimed.

- Refine the survey research prototype for state-dependent behavior and verified
  citation ranges. Add two frozen synthetic transfer cases, counterbalanced runs,
  executable state oracles and saved method snapshots. Document remaining citation,
  cache identity and delivery-guarantee failures; onboarding stays unchanged.

- Add an as-built survey research method and read-only synthetic comparison
  harness, with offline citation checks and explicit human semantic review.
  Initial results do not justify replacing architect onboarding; no production
  surveyor, extra memory or automatic research is enabled.

- Fix architect onboarding from the offer dialog: dispatch the slash command via
  the host command API instead of submitting it as a model prompt. Regression
  tests cover host command dispatch and failure without a prompt fallback.

- Experimental opt-in project workflow coaching, with explicit onboarding,
  review and retention proposals for one compact project skill. Reuses canonical
  docs and selectively loaded references/helpers; exact diffs and user approval
  precede persistent workflow changes. Instruction-only, with no automatic memory
  writes, transcript mining, helper execution or additional Jev requests.

### Added

- Native Claude Code tools, `search_code` and `ask_file`, wrapping the existing
  semantic search and file-question commands. Includes bounded search output,
  batched questions and navigation guidance for main agents and subagents.
- Compact agent search presentation with a configurable eight-file shortlist,
  shared source budget and explicitly partial, declaration-focused snippets.
- PHP declaration parsing for methods, functions, class headers and members.
  Invalid PHP syntax falls back to text selection; parser-aware caches are versioned.
- Experimental, opt-in user approval after a recognized Auto mode denial.
  Approval applies to one retry of the same call and arguments; it does not
  persist permission rules. Retry after a real classifier denial remains
  unverified in live testing.
- Opt-in permission diagnostics that log decisions without changing them.
- Public source installation instructions and plugin configuration documentation.
- Rust test workflow for the public repository.

### Changed

- Documentation emphasizes Claude Code as the primary integration. Codex and
  jcode retain launcher support; broader integration is planned separately.
- README reorganized around installation, core workflows, the architect and
  plugin configuration. Detailed documentation moved to `docs/reference.md`.
- Architecture documentation updated to describe the current implementation.
- Public distribution excludes local configuration and generated build archives.

### Fixed

- Added new runtime hook modules to the installer’s embedded file inventory.
- Restricted legacy private Homebrew publication steps to their original repository.

## 0.21.0 — 2026-10-06

### Fixed

- Cold-cache dialogs always offer **Compact and continue**, including when a
  follow-up is classified as new work. Routing now considers recent session context.

### Changed

- Architect hooks notify the coding agent rather than spawning review agents.
  The coding agent reviews a small case itself or starts the architect through
  the normal agent workflow. Removed the dedicated review queue and review dialog.

## 0.20.0 — 2026-10-03

### Added

- Repository architect with onboarding, architecture memory, decision records,
  change classification and baseline audits.
- Configurable architect modes: `advise`, `shadow` and `off`.
- Experimental cost-awareness notes for main agents and subagents, disabled by default.

### Changed

- New-work handoffs use **Clear and continue** rather than requiring a fresh process.
- Turn-folding hooks filter for completed answers.

## 0.19.0 — 2026-10-02

### Added

- Large-context checks for subagents and an option to set the shared auto-compaction window.
- Shadow effort routing and optional application of lower effort recommendations.
- Optional turn folding that shortens read-only tool output after a completed turn.

### Fixed

- Usage analysis uses the highest output-token count across streamed response blocks.

### Changed

- Refined the optional robot output style’s wording.

## 0.18.0 — 2026-10-02

### Added

- Weekly usage-limit reset information in the status line.

### Fixed

- Cold-cache analysis respects the one-hour cache lifetime.
- Restart cost estimates account for fast-mode pricing.
- Added pricing for Claude Sonnet 5.5.
- Invalid numeric environment settings fall back to their defaults.

### Changed

- Centralized model pricing in `src/pricing.rs`.

## 0.17.1 — 2026-10-02

### Changed

- Increased the default large-context threshold to 375,000 tokens while
  preserving explicit user configuration.

## 0.17.0 — 2026-10-02

### Added

- Explicit credential management through `auth login`, `auth status` and `auth logout`.
- Hidden key entry, macOS Keychain storage and standard-input support for automation.

### Changed

- Made the 1Password fallback explicitly opt-in through `SESSIONKIT_OP_REFERENCE`.

### Fixed

- Included image-token estimates in tool-usage analysis.

## 0.16.1 — 2026-09-30

### Added

- Context-cost notes for agents working with large contexts.

### Fixed

- Setup output lists the installed plugin features.

## 0.16.0 — 2026-09-30

### Added

- Optional `sessionkit-robot` output style.
- Large-context compaction dialog and configurable auto-compaction window.
- Retry of eligible edits refused because the target file has not been read.
- Deduplication of repeated teammate reports in idle notifications.
- Optional outline-first reads for large files.
- `sessionkit peers` for locating relevant warm sessions.
- Warm-peer information in shadow agent-routing logs.
- Tool-output and repeated-context analysis through `usage --tools`.
- Measurement commands for tool-output trimming and outline-first reads.

## 0.15.0 — 2026-09-30

### Added

- Agent notification after successful manual SessionKit compaction.

### Fixed

- Cache-state checks use the post-compaction context rather than the previous call.

## 0.14.0 — 2026-09-30

### Added

- New-work routing in cold-cache dialogs.
- Shadow model and effort recommendations for subagents.

## 0.13.1 — 2026-09-30

### Fixed

- Context selection retains all relevant assistant text within a turn, including
  questions, options and plans. Oversized conclusions retain both their opening
  and ending sections.

## 0.13.0 — 2026-09-29

### Changed

- Revised compaction output-retention questions and thresholds.
- Added dataset-based compaction evaluation with separate training and test sets.

## 0.12.0 — 2026-09-29

### Added

- A file-question reminder after the first large read in each agent loop.
- `sessionkit --version`.

### Fixed

- Unknown options produce an error rather than starting an agent.
- Setup descriptions cover all plugin hooks.

## 0.11.0 — 2026-09-29

### Added

- Interactive confirmation before expensive cold-cache continuation.

### Changed

- Semantic search verifies source snapshots before sending requests or using cached results.
- Increased the TypeSafe request timeout for semantic search.

## 0.10.0 — 2026-09-29

### Added

- `sessionkit skills` for identifying recurring workflows and drafting reusable skills.

## 0.9.1 — 2026-09-28

### Fixed

- File questions include supporting source lines for answers that are not clearly negative.

## 0.9.0 — 2026-09-28

### Added

- Semantic code search through `sessionkit grep`.
- File questions, filtering and line discovery through `sessionkit ask`.
- Verbatim conversation compaction and Claude Code plugin integration.
- Unified SessionKit skill for search and file questions.

### Changed

- TypeSafe retries respect server retry guidance and use jitter.
- Context selection skips duplicate messages written by compaction.

### Fixed

- Legacy Homebrew installation compatibility with sandboxed installation steps.

## 0.8.0 — 2026-09-28

### Changed

- Reimplemented SessionKit as a single Rust binary without a Node.js runtime.
- Added the original Homebrew distribution path and relocated evaluation results.

### Fixed

- Unknown model prices no longer cause status-line failures.
- Forked-call deduplication uses deterministic transcript ownership.

## 0.7.0 — 2026-09-27

### Added

- `sessionkit next` and a zsh handoff hook for fresh-session continuation.

### Changed

- Cold-session handoffs can continue work in a fresh session in the same terminal.

## 0.6.0 — 2026-09-27

### Added

- Subscription usage limits and estimated per-call limit consumption in the status line.

### Changed

- Standardized context-size labeling.

## 0.5.0 — 2026-09-27

### Added

- `sessionkit setup` with backups, preservation of unrelated settings and removal support.
- Original npm distribution path.

## 0.4.0 — 2026-09-27

### Added

- Claude Code status line with context size, per-call cost and cache state.
- Resume warnings and cold-cache checks through settings hooks.

## 0.3.0 — 2026-09-27

### Added

- Local usage analysis and context-selection evaluation.
- Preflight cost estimates and spending confirmation.
- Session-chain tracking for fresh-session continuation.

### Changed

- Renamed `jev-start` to `sessionkit` and introduced subcommands.
- Updated context-selection thresholds and capped retained open items.

## 0.2.0 — 2026-09-27

### Added

- Automatic continuation-versus-search routing.
- Prioritized rules, preferences and open items in continuation context.
- Repository-change information in session handoffs.

### Changed

- Search falls back to all projects when the current directory has no sessions.
- Numbered retained turns for traceable context references.

## 0.1.0 — 2026-09-27

### Added

- Search and continuation of earlier Claude Code sessions using verbatim selected context.
- Session selection by identifier, title or interactive picker.
- Launch integrations for Claude Code, Codex and jcode.
- TypeSafe authentication, dry-run output and verbose selection diagnostics.
