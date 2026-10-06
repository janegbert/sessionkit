---
name: project-workflow
description: Experimental, user-invoked onboarding or review of one compact project workflow skill, with selectively loaded references and reviewed helpers. Proposes exact diffs rather than automatically storing lessons or memories.
argument-hint: onboard | review | propose [lesson]
disable-model-invocation: true
---

# Compact project workflow (experimental)

This command helps maintain one project-specific workflow entrypoint, not a
collection of lessons. It is available explicitly even when the projectWorkflow
plugin option is off; that option controls automatic coaching only.

Read `GUIDE.md` for the requested operation. Read `TEMPLATE.md` only when drafting
an entrypoint. Resolve these files relative to this plugin skill's directory.

- **onboard**: inspect the project's existing skills and authoritative docs;
  propose a small entrypoint or improve an existing one, without writing yet.
- **review**: inspect the existing workflow package for duplication, stale links,
  unsafe helpers and excessive detail; propose consolidation/deletion.
- **propose [lesson]**: test whether a durable recurring lesson merits retention.
  Usually change nothing. Prefer a small edit to an existing source or skill.

For every operation, show an exact diff, destination and evidence, then wait for
explicit user approval before writing. Invocation requests a proposal, not blanket
permission to save it. A subagent returns a proposal to the parent and never
persists workflow knowledge. No transcript mining, automatic memory writing,
external research, new paid requests or helper execution is required by this skill.
