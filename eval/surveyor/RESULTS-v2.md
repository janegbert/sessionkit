# Survey method v2: transfer and regression results

2026-10-06; Claude Code 2.1.291, Sonnet 5.5, low effort. Research only: production
onboarding, architect and stored baselines are unchanged.

## What changed before evaluation

TRACE.md now follows payload vs identifier, revision/cache key, pending/completed
and acknowledgement/failure state, plus configured dynamic imports. REPORT.md
requires checking quotes against actual numbered lines, rereading uncertain ranges
and leaving call counts to instrumentation. Method, schema, source and rubric
hashes are captured per run. The rubric was frozen before the model runs.

Two newly authored, less signposted cases test transfer rather than repeating the
old SQLite example:

- **snapshot**: jobs copy payloads; later retitle does not alter that snapshot;
  imported RemoteStore is not registered; environment overrides file settings.
- **revision**: jobs hold identifiers; worker reads current state through a
  revision-keyed cache; transport activation uses a configured import string;
  acknowledgement determines whether work stays pending.

They are targeted fixtures authored by the method's implementer, **not an
independent held-out corpus**. Both are small Python examples, with five rubric
items each. Executable offline oracles verify state transitions without HTTP.
Both prompt conditions use the same schema, tools, effort and source. Two
repetitions counterbalance order within and across cases (eight model runs).
No method tuning took place between those eight runs.

## Measured results

| Case | Repeat | Condition | Source-tool calls | Tool-output chars | Citation checks | Claude dollars |
|---|---:|---|---:|---:|---|---:|
| snapshot | 1 | legacy | 8 | 2,292 | pass | 0.045991 |
| snapshot | 1 | survey | 10 | 6,629 | pass | 0.055544 |
| revision | 1 | survey | 10 | 7,274 | fail | 0.060152 |
| revision | 1 | legacy | 9 | 3,024 | pass | 0.047573 |
| snapshot | 2 | survey | 10 | 6,629 | pass | 0.057408 |
| snapshot | 2 | legacy | 8 | 2,292 | pass | 0.045631 |
| revision | 2 | legacy | 9 | 3,024 | pass | 0.052858 |
| revision | 2 | survey | 10 | 7,274 | pass | 0.057810 |

Citation validity: legacy 4/4, candidate 3/4. The candidate's first revision report
uses bootstrap.py lines 13-29 (17 lines) for c1, exceeding the shared 12-line
maximum; the quote itself is present. The validator rejects it, never silently
repairs it. There are no plugin/tool contamination or fixture mutation errors.

Means over these four runs per condition: legacy 8.5 source-tool calls, 2,658
output characters and $0.048013; candidate 10 calls, 6,951.5 characters and
$0.057729. The additional reference reads have a visible context cost. Dollar
estimates are API-equivalent Claude costs, not subscription billing, and include
cache/order effects. This tiny sample is not a statistical cost result. No Jev
requests, runtime execution or browsing tools were offered.

## Semantic review and remaining counterexamples

Review here is the implementing assistant's source-based analysis, with executable
oracles where applicable; it is not a blinded, independent expert assessment.
There is deliberately no automatic aggregate 'truth score'.

- Both conditions correctly distinguish copied snapshots, unregistered imported
  backends, environment precedence and dynamically loaded transport. The candidate
  explicitly explains pending-vs-completed publication (revision c4 in both runs);
  the legacy accounts are less explicit (revision c3/c4 in run 1, c8 in run 2).
- Both reason about revision-keyed caching in the ordinary submit/retitle path.
  **Neither flags identity reuse.** Repository.submit overwrites an existing key
  and resets revision to 1. If a revision-2 payload is already cached, resubmitting
  and retitling that key can produce revision 2 again and serve the earlier payload.
  Broad 'new revision always gives fresh data' conclusions therefore need a
  unique-identity/monotonic-revision precondition not guaranteed by this source.
- An executable diagnostic for identity reuse was added **after** the runs. It
  demonstrates the error without altering fixture source or increasing the frozen
  ten-item rubric's denominator. It is a next-iteration regression, not evidence
  that these runs passed an additional pre-registered check.
- Both still use 'at-least-once' too loosely in places. Retaining a job on negative
  acknowledgement gives retry eligibility, not guaranteed delivery without a
  continuing driver, durable state and success assumptions.
- File-count/completeness self-reports remain unreliable. Several snapshot reports
  say six files although seven exist. The candidate better avoids guessed tool
  counts, but source enumeration is still not proof of complete behavior.

Questions generally distinguish intent from observable facts. Some are bundled
or prematurely frame prototype behavior as a production guarantee. No human
calibration answers were provided and no norms/baselines were confirmed.

## Original-development regression (not held-out evidence)

A further pair used the original ten-file SQLite fixture with the same v2 method:
legacy 11 source-tool calls, $0.058901; candidate 13, $0.070188. Both pass citation
checks. The candidate now explicitly distinguishes rename before polling (new
text can publish) from rename after completion (no new enqueue), in c6. The
legacy still omits that distinction in its rename account (c7). This addresses
the earlier observed failure, but is a development regression, not proof of
superiority on unseen projects. The candidate still overstates 'at-least-once'
in a failure discussion despite acknowledging a required restart policy.

## Decision

**Do not promote or reroute onboarding.** The focused state reasoning improved,
but citation reliability, unsupported guarantees and identity-lifecycle reasoning
still fail the desired bar. Costs/context are higher on these cases, and no
independent corpus or accepted-baseline/calibration round trip has been tested.

Next iteration should check create/re-create/delete identity lifecycle and revision
reuse, distinguish retry eligibility from delivery guarantees, and use deterministic
citation validation as an acceptance gate rather than trusting self-check prose.
Only then compare on an independently reviewed, broader corpus (including other
languages and real configuration patterns), keeping original failures visible.

Raw streams, reports, method snapshots and full metadata remain local. The repo
contains only synthetic source, oracles, methodology and this summarized analysis.
Use `run.py --case transfer --variant both --repeat 2` to reproduce the protocol;
model answers/timing/costs may differ. For these first eight runs, method snapshots
were backfilled locally only after verifying their hashes against captured metadata;
future runs save them automatically before the model starts.
