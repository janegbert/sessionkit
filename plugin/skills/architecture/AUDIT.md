# Audit

You compare the repository with the baseline in `ARCHITECTURE.md`: what changed architecturally
since then. The question is the delta; a codebase that was imperfect at the baseline and still is
in the same way is no finding.

## 1. Collect the delta

- The baseline commit is in the first line of the memory. `git log --oneline <baseline>..HEAD`
  and `git diff --stat <baseline>..HEAD` give what changed; read the diffs of manifests, schemas
  and migrations, new top-level folders, and imports added between modules.
- `~/.cache/sessionkit/architect-checks.jsonl` holds every change the hooks classified, one JSON
  object per line with `root`, `file`, `relevance` and `wake`. The lines of this repository since
  the baseline date point at the changes Jev found architectural.
- Check each statement in the memory against the code now: every owner, every dependency rule,
  every decision, every known debt entry.

Done when every section of the memory has been checked and every changed module has been looked at.

## 2. Report

Group the findings, each with evidence as `path:line` or a commit:

- Boundaries that weakened
- New dependencies between modules, and new external dependencies
- Responsibilities or data ownership that moved
- Statements in the memory that are no longer true
- New debt
- Debt that shrank or disappeared
- Decisions that no longer match the code

Leave out a group with nothing in it. Then list the memory edits you propose, numbered, each one
line: what changes and why. Change no file in this step.

## 3. Apply

You get the accepted numbers in a later message. Apply those edits, consolidate the memory while
you are in it, and move the baseline line to the current commit only when the message says so.
Answer with what you changed.
