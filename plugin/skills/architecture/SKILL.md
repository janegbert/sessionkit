---
name: architecture
description: The project's architect and its memory, ARCHITECTURE.md. `onboard` surveys the repository, calibrates the few assumptions that matter with the person and sets the baseline; `audit` reports what changed architecturally since the baseline; `record` writes decisions made in this session into the memory.
argument-hint: onboard | audit | record [decision]
disable-model-invocation: true
---

# The architect

sessionkit gives the repository an architect: the agent type `sessionkit:architect`, with its own
context and the memory in `ARCHITECTURE.md` at the repository root. Once onboarded, sessionkit's
hooks send each plan and edit to Jev; for a change that is architectural, a note starting with
`sessionkit architect (Jev)` arrives in your context, and you check it or start the architect.
You orchestrate here; the architect reads, judges and writes the memory. Give it the repository
root and the mode, never your own reasoning about the code: its value is a separate view.

The steps the architect follows sit beside this file: `ONBOARD.md` and `AUDIT.md`. Read the one
you need and put its whole text in the architect's prompt: the plugin folder lies outside the
repository, where the architect may not be allowed to read.

## onboard

1. Look for `ARCHITECTURE.md` at the repository root. One with the line `<!-- sessionkit architect`
   is already onboarded: tell the person and offer `audit` instead. One without it was written by
   people: the architect takes it as input and keeps what it says.
2. Spawn `sessionkit:architect` in the foreground with the prompt `Onboard. Repository: <root>.`,
   a blank line and the text of `ONBOARD.md`. Keep its agent id. It writes a draft memory
   and answers with its calibration questions, most consequential first, each with its
   recommended answer and the evidence.
3. Pipe the architect's answer through `sessionkit architect rank <root>`. Jev reads the cited
   code beside each recommended answer and the command returns the questions reordered, each
   with a line `Code: …`: first where the code contradicts the recommendation or a citation does
   not exist, then what only intent can settle, last what the code already shows. Use that
   order; when the command fails (no TypeSafe key), keep the architect's order.
4. Put the questions to the person with AskUserQuestion, in rounds of at most four, the
   architect's recommendation as the first option; four is the most AskUserQuestion takes at
   once. Mention a `contradicts` or missing citation in the question, so the person knows the
   draft and the code disagree. When `mattpocock-skills:grilling` is
   installed, its rounds are the model: decisions go to the person, facts you look up yourself.
   Done when every question has an answer or the person chose to leave it open.
5. Send the answers to the architect with SendMessage, by its agent id: `Calibration answers:
   <question: answer, one per line>. Apply them and set the baseline.` It updates the memory and
   writes the baseline line with the current commit.
6. Show the person the path, the counts the architect reports (confirmed, observed, inferred,
   open, known debt) and suggest committing `ARCHITECTURE.md`. From then on the hooks are active
   in this repository; the plugin option *Architect* in `/config` sets them to advise, shadow or off.

## audit

1. Spawn `sessionkit:architect` in the foreground: `Audit. Repository: <root>.`, a blank line and
   the text of `AUDIT.md`. Keep its agent id. It answers with the delta since the baseline and
   the memory edits it proposes; it changes nothing yet.
2. Show the person the report as the architect wrote it and ask which proposed edits to accept,
   with AskUserQuestion when there are few, as a numbered list when there are many.
3. Send the choice to the architect with SendMessage: `Apply: <numbers>. Move the baseline: <yes
   or no>.` Move the baseline only when the person accepted the audit as the new starting point.

## record

Spawn `sessionkit:architect`: `Record. Repository: <root>. Decisions: <each decision in one line,
with the files it touched>`. Take the decisions from the arguments, or from the `sessionkit
architect` lines of this session marked as decisions. The hooks do this by themselves at the end
of a turn; use it when they are off or when the person names a decision afterwards.
