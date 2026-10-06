# As-built survey: candidate method, not a proven replacement

Status: research/evaluation prototype. Architecture onboarding still uses the
existing architect. No surveyor is installed, no baseline is rewritten, and no
additional project memory or automatic research loop is introduced.

## Research basis

The consulted primary references are Gernot Starke's arc42 documentation:

- [Context and scope, section 3](https://docs.arc42.org/section-3/): delimit the
  system and its communication partners; separate business from technical context.
- [Building blocks, section 5](https://docs.arc42.org/section-5/): map abstractions
  to code, zoom selectively into consequential blocks, favor relevance over detail.
- [Runtime view, section 6](https://docs.arc42.org/section-6/): use a representative
  selection of architecturally important scenarios, including operations and errors.
- [Deployment, section 7](https://docs.arc42.org/section-7/): connect artifacts to
  infrastructure and distinguish environments.

Consulted 2026-10-06. These are citations and our own synthesis, not copied arc42
text/templates (the source documentation is CC BY-SA 4.0). This does not evaluate
all reconstruction methods. C4 is a presentation candidate, not evidence that an
architecture claim is correct; its site could not be retrieved in this pass.
SEI reconstruction guidance needs a verified source before a comparative claim.

Our additions below—counterexample checks, evidence standings, bounded discovery
and evaluation gates—are hypotheses to test, not claims that arc42 prescribes
this particular agent workflow or that it is already the best method.

## The procedure

1. **Scope and constraints.** Establish repository root, snapshot, purpose and
   exclusions. Separate code/config evidence from observed runtime behavior.
   Do not execute programs, browse, query external services or inspect secrets
   simply to complete a survey.
2. **Breadth first.** Read dense manifests, launch/deployment configuration and
   existing docs. Find important entrypoints, blocks, stores and external interfaces.
   Directory names, dependencies and diagrams are leads, not verified topology.
3. **Representative slices.** Trace one primary request, one job/operational path
   where present, and one boundary/error path likely to disprove the initial map.
   Identify who invokes what, how data moves and where writes actually occur.
4. **Try to disprove.** Track payload vs identifier, pending/completed/failure
   states, cache revisions and acknowledgement conditions before making behavioral
   claims. Verify citation ranges against actual numbered source, not memory.
   Follow configuration precedence and alternate entrypoints;
   search for direct writes bypassing a claimed owner, inactive adapters and
   contradictory docs. A declared dependency is not proof of active use. Searches
   support scoped negative findings, not universal absence claims.
5. **Evidence-led draft.** Every consequential claim has exact source locations,
   its standing (observed source, inference or unknown) and limitations. As-built
   state does not become a norm. Code cannot establish intentional debt, rationale
   or what is running in production without appropriate additional evidence.
6. **Calibrate intent only.** Ask a small number of questions whose answers change
   later architectural judgments. Do not ask people to resolve facts obtainable
   from inspected source. Preserve unknowns instead of manufacturing certainty.

Stop once important paths and boundaries have support and counterchecks, or the
agreed budget is exhausted. Report exclusions and remaining uncertainties. Never
claim that every dependency is known merely because a folder scan is complete.

## Compact specialist shape

The evaluation uses `eval/surveyor/SURVEY.md` as the short role/procedure, with
`TRACE.md` for tracing/counterchecks and `REPORT.md` for standing/calibration.
Read those references only at their relevant stage. These are research prompts,
not new user-discoverable skills. The eventual surveyor should return a proposal
for the same ARCHITECTURE.md the architect uses, not keep a parallel knowledge base.
The parent owns calibration/acceptance and baseline persistence.

## Evaluation and promotion

See [the evaluator](../eval/surveyor/README.md). Compare the first four stages of
the current onboarding instructions with the candidate procedure under the same
read-only task/output contract, tool set, model and effort. The comparison is
therefore an adapted prompt comparison, not a full old-versus-new onboarding test.

Judge coverage, grounded/correct claims, counterevidence, useful intent questions,
unknowns and tool/context/cost usage. Check citation mechanics automatically;
judge semantics separately. A valid quote does not establish that a claim follows.
Use independent repetitions and more repositories before claiming improvement.

Promotion requires a reviewed held-out set, no increase in unsupported/normative
claims, non-regressed important-path coverage and acceptable combined cost/context.
The [v2 results](../eval/surveyor/RESULTS-v2.md) improve the original conditional
rename explanation, but show higher overhead, a citation failure and an unhandled
identity-reuse cache case. These are targeted author-built transfer examples, not
an independent held-out corpus; semantic review was by the implementing assistant.
The evidence still does not justify promotion.

Also test the eventual accepted baseline and user calibration round trip. Until
then, do not reroute onboarding, register a production surveyor, increase memory
files or advertise the method as superior.
