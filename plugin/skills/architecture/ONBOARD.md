# Onboard

You are building the first architectural memory of this repository: the baseline every later
review measures against. A memory that states a guess as intent is worse than no memory; the
standing of each statement matters as much as the statement.

## 1. Survey

Build a picture of the system as it is. Start from what is cheap and dense: the manifests and
workspace files, the folder layout, entry points, README and existing docs (`docs/adr/`,
`CONTEXT.md`, a hand-written `ARCHITECTURE.md`), then the imports between top-level modules.
`git log --stat` shows hot spots and the areas that change together.

Cover, where the repository has them: modules or domains and their responsibilities, the
direction of dependencies between them, entry points, persistence and who writes which data,
infrastructure and external services, public interfaces and contracts, shared or global state,
orchestration of processes and jobs, package and test boundaries, build and deployment topology.

Done when every top-level module has a one-line responsibility and every dependency between
modules is known in direction.

## 2. Audit

Look for: the architecture the code seems meant to have, strengths worth keeping, unclear
boundaries, concepts that exist twice, coupling hotspots, dependencies against the apparent
direction, and inconsistencies that look historical. For each, note the evidence as `path:line`.

## 3. Separate what you see from what was meant

For every statement of ownership, rule or intent, decide its standing: `[observed]` when the code
shows it, `[inferred 0.N]` when you read intent into it, with your confidence. A cycle that exists
is observed; that it is accepted debt is inferred until a person says so. Where you cannot tell
intent from accident, write it under Open questions with the evidence for each side.

## 4. Choose the calibration questions

Ask about an inference when a wrong guess would steer many later reviews wrong: ownership of a
central concept, a dependency rule that many changes will meet, two similar concepts that may or
may not be the same, debt versus intended design. An inference whose consequences stay inside
one module is no question: it keeps its standing, and an audit can raise it later. Each question
is answerable with a choice. Order them by how many later reviews a wrong guess would affect,
most first.

## 5. Write the draft

Write `ARCHITECTURE.md` in the shape your instructions give, with the first line
`<!-- sessionkit architect: baseline pending -->`. Known debt found here gets an `AD-NNN` entry,
`Pre-existing at onboarding`, and a policy: no warning on unrelated changes, a warning when a
change widens it. Existing ADRs in `docs/adr/` go under Decisions as one line each; write no new
ADRs during onboarding.

Answer with the questions, numbered, each as:

```
N. <question>
   Recommended: <answer>  (confidence 0.N)
   Evidence: <path:line, one or two>
   Options: <choice>; <choice>; ...
```

## 6. After calibration

You get the answers in a later message. Apply each: an answered question becomes `[confirmed]`
or changes the statement; one left open stays under Open questions. Then replace the first line
with `<!-- sessionkit architect: baseline <git rev-parse --short HEAD> <YYYY-MM-DD> -->`. Answer
with the counts of confirmed, observed, inferred and open statements and of known debt.
