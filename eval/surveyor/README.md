# As-built survey evaluation prototype

This tests a candidate reconstruction procedure before changing onboarding.
The production architecture skill/architect are unchanged. Method research and
promotion gates: [docs/survey-method.md](../../docs/survey-method.md).

## Corpus and contract

The original development fixture is an artificial, ten-file Python repository
with eight labeled checks in rubric.json:
multiple launch paths, a request path, a worker/external interface, direct-write
bypasses, configuration precedence, stale docs, a declared unused dependency and
an unconnected prototype. The README explicitly warns it is historical: this is
an easy diagnostic fixture, not a held-out benchmark or eight independent repos.

Two additional, less signposted transfer cases live in cases/snapshot and
cases/revision (five frozen rubric items each in cases/rubric.json). They contrast
copied jobs with identifier jobs, revision caching and dynamic activation. They
are targeted author-built fixtures, not an independent held-out corpus. Offline
oracles check ordinary transitions and a later-discovered identity-reuse failure.

The legacy condition uses stages 1-4 of the current ONBOARD.md. The candidate uses
SURVEY.md, with TRACE.md and REPORT.md read at their relevant stages. Both get the
same read-only scope, JSON report schema, source tools, model, effort and budget.
This adapts the old workflow: no actual calibration, writes or baseline acceptance
are tested. Shared evidence instructions already strengthen the legacy prompt.

## Offline tests (no key/network)

```sh
python3 -m unittest discover -s eval/surveyor -p 'test_*.py' -v
```

Sixteen tests cover schema/standing, fabricated quotes, line ranges,
traversal/symlink escapes, missing evidence, duplicate IDs/stream rows,
counterbalanced scheduling, fixture selection and executable state oracles. Valid quotes are
explicitly **not** treated as a score for architectural truth.

## Optional paid model comparison

Requires Claude Code and its normal authentication. This deliberately makes paid
Claude model requests (API-equivalent cost also reported for subscriptions):

```sh
python3 eval/surveyor/run.py --variant both --model sonnet --effort low
# Counterbalance order across repetitions; not a substitute for held-out repos:
python3 eval/surveyor/run.py --case transfer --variant both --repeat 2
# Select one case: development (default), snapshot or revision; all runs all three.
python3 eval/surveyor/run.py --case revision --variant survey
```

The runner copies only the selected synthetic case into a fresh temporary project, never gold
labels or evaluation files. It places just the method references beside it. It
turns off auto-memory, explicitly disables installed plugin IDs, restricts tools
to Read/Glob/Grep, disables session persistence and MCP, and detects unexpected
plugins/tools and changed fixture files. Native host behavior is not sandboxed;
this is not an OS-level filesystem boundary. Do not pass client code to this toy
runner. No Jev tools, runtime execution or browsing are offered.

Default timeout is 180 seconds and Claude budget $2 per run. Bounds are CLI-level,
not a guarantee on billing; a timeout/cutoff is an incomplete run. Output defaults
to a private system temporary directory, not committed eval/results. Reusing an
existing run directory fails rather than overwriting evidence.

Artifacts per run: prompt.txt, output.jsonl, stderr.txt, report.json, metadata.json,
and candidate method snapshots under method/.
Raw streams may include host metadata; keep them local, do not publish them.
Metadata records exact fixture/prompt hashes, version, resolved model usage,
source-tool counts, output characters, elapsed time and Claude dollar estimates.
Method/schema/rubric hashes identify the frozen evaluation version; gold labels
are hashed locally but not passed to the model.
Temporary source paths make the prompt hashes differ across runs; saved prompts
and source hashes preserve what each run actually saw. Tool counts are measured
from unique stream call IDs, not the model's self-report.

```sh
python3 eval/surveyor/check.py /path/to/report.json
# For a transfer case, use the corresponding source root:
python3 eval/surveyor/check.py /path/to/report.json --root eval/surveyor/cases/revision
```

Exit 0 means citation/structure checks pass, **not** a semantically correct survey.
Exit 2 indicates invalid/incomplete/contaminated output. check.py is a local
validator of model output; it does not repair a report or silently rerun a model.

## Human semantic review

For each rubric fact, mark supported / partial / missed / wrong and identify
claims and evidence. For every claim, distinguish factual support, sufficient
citations and appropriate standing. Search for counterexamples (including state
preconditions), not just matching words. Classify questions as useful intent,
factual, bundled or unsupported. Do not let a good quote, a convincing diagram
or a single aggregate number hide unsupported assertions.

### Latest iteration

[The v2 comparison](RESULTS-v2.md) has eight counterbalanced transfer runs and an
original-development regression pair. The candidate now states the conditional
rename behavior explicitly, but still fails one citation check and misses an
identity-reuse cache counterexample. **Onboarding remains unchanged.** The
semantic review is the implementing assistant's source/oracle analysis, not an
independent expert assessment. No automatic truth score is inferred.

### Historical first clean pair, 2026-10-06

Claude Code 2.1.291, resolved model Sonnet 5.5, low effort, one run per condition:

| Measure | Adapted legacy | Candidate survey |
|---|---:|---:|
| Source-tool calls | 11 | 13 |
| Tool-output characters | 4,457 | 7,478 |
| Elapsed seconds | 25.18 | 29.43 |
| Claude API-equivalent dollars | 0.060845 | 0.068283 |
| Citation mechanics | pass | fail: wrong range on one Compose quote |

Qualitative review against source:

- Both identify the important launch paths, SQLite jobs, direct-write bypasses,
  environment override, stale docs and inactive prototype. The candidate makes
  the ordinary request flow more explicit; neither has a demonstrated recall win.
- The candidate scopes runtime/inactive-use uncertainty more carefully, but its
  third question bundles prototype plans and actual CLI deployment. The legacy
  questions are more focused on ownership and index consistency.
- Both overgeneralize the rename effect. Rename does not enqueue a new job, but
  an **existing pending job** joins the current item text and can publish its
  renamed value. Neither supports a blanket 'renamed items are never published'
  conclusion. This counterexample is not one of the eight original rubric labels;
  it is a discovered error, not a post-hoc increase to their coverage denominator.
- Both self-report tool counts incorrectly. The measured stream counts above are
  authoritative. Reading every file also does not establish complete behavior.
- The candidate's extra reference reads add context in this tiny project. Costs
  also depend on cache/order; this single pair cannot establish a cost effect.

An earlier pair was rejected because plugins still loaded despite empty setting
sources. The runner now disables installed plugin IDs explicitly and rejects any
remaining contamination. Those runs are not counted as comparison evidence.

**Conclusion: do not promote or reroute onboarding yet.** The candidate is not
proven better and fails the citation gate on this pair. Next work: verify exact
ranges, check behavior preconditions instead of broad negative conclusions, avoid
model-reported counts, and build a genuinely held-out, less signposted corpus.
Repeat/counterbalance before drawing claims about precision, coverage or cost.
No raw model transcript or user conversation is included in this repository.
