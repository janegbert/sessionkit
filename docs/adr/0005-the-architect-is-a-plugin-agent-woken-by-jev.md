# ADR 0005: Architect reviews are initiated by the coding agent

## Context

Architectural review requires repository memory, change classification and a
reviewer with an independent context. Hook-initiated agent spawns can encounter
permission restrictions in Claude Code Auto mode. Review advice should remain
separate from permission enforcement and must not veto ordinary edits.

## Decision

Implement the architect as a Claude Code plugin agent, with onboarding and audit
skills. The hook sends eligible plans, edits and writes to the Rust classifier
asynchronously. The classifier evaluates the change against `ARCHITECTURE.md`
and the individual decision records in `docs/adr/`.

For a relevant result, the hook adds a note to the originating coding agent's
context. The note identifies the change, its classification and any applicable
decision records. The hook does not spawn a review agent.

The coding agent reviews a small case directly or starts `sessionkit:architect`
through its normal agent workflow. Architectural decisions and memory changes
require user approval. Architecture memory and the baseline remain in
`ARCHITECTURE.md` and `docs/adr/`, rather than a separate review-state database.

## Consequences

- Reviewer creation remains subject to the normal agent permission workflow.
- The reviewer receives the change and repository evidence without relying on
  the coding agent's justification.
- Classification, review orchestration and authorization remain distinct concerns.
- Hooks do not need a reviewer queue or a separate review-verdict dialog.
- Notes delivered after a turn's final request may be read on the next turn.
