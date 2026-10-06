# Surveyor with native SessionKit tools: availability is not adoption

2026-10-06; Claude Code 2.1.291, Sonnet 5.5, low effort. The survey procedure,
report schema and source are held fixed within each pair. Production onboarding
and user plugin configuration are unchanged.

## Treatment

`--tool-profile none` offers Read/Glob/Grep. `sessionkit` additionally loads a
session-local minimal plugin containing only the production search-tools.js,
including its schemas and navigation guidance. It does not load index.js,
architect, auto-memory, permission, effort or other experiment hooks. Native
search_code and ask_file invoke the real installed CLI through a telemetry proxy.
The proxy rejects sources outside the synthetic project and unexpected CLI flags.
This is not an OS sandbox; normal Read still accesses the method references.

The shared procedure permits explicitly offered inspection tools under their
normal disclosure/permission rules. The treatment is tools **plus their existing
navigation guidance**, not a schema-only ablation. No test forces tool usage
except the separately labeled dispatch smoke below.

## Unforced paired comparison

Two diagnostic cases, one pair each, with reversed order between cases:

- development-large: the original SQLite case, with 3,000 inert comment lines
  prepended to api.py and storage.py. Behavior is unchanged. This is a deliberately
  artificial read-size test, not a larger realistic repository or held-out case.
- revision: the existing identifier/revision-cache case, unchanged.

| Case | Profile | Source-tool calls | Tool-output chars | Native calls | Citation checks | Claude dollars |
|---|---|---:|---:|---:|---|---:|
| development-large | none | 13 | 48,722 | 0 | pass | 0.206100 |
| development-large | sessionkit | 13 | 48,722 | 0 | pass | 0.192195 |
| revision | sessionkit | 10 | 7,274 | 0 | pass | 0.061793 |
| revision | none | 10 | 7,274 | 0 | pass | 0.058379 |

Both native tools appear in the advertised inventory for both treated runs.
Nevertheless, the agent chose only conventional inspection. The treatment adds
schemas/guidance but does not demonstrate autonomous adoption or a read reduction.
No CLI/Jev calls occurred in these four runs; combined estimated cost equals the
reported Claude estimate. Dollar differences can reflect cache/order/randomness,
not native tool execution, and there is no statistical savings claim.

The implementing assistant reviewed the structured reports against synthetic
source; this is not a blinded expert assessment or automatic truth score. Both
profiles describe the pending/completed SQLite rename distinction. The untreated
revision run notices a possible revision reset/cache collision on re-submission;
the treated run does not. As neither invokes a native tool, this difference is
not evidence that a Jev answer improves or damages the survey. Citation validity
does not establish architectural correctness or delivery guarantees.

A prior unforced development pilot also advertised both tools but invoked neither;
it failed one quote-location check. It is a pilot, not included in the paired table.

## Separate forced dispatch smoke

`--dispatch-check` explicitly requests one search_code call and one ask_file call
before surveying the small development case. This validates dispatch, **not**
spontaneous selection, quality or cost savings:

- Both actual native tool calls reach the real CLI and exit successfully.
- CLI grep and ask are each called once; no synthetic-root guard denies occur.
- Citation checks pass; fixture hashes remain unchanged.
- Claude API-equivalent estimate: $0.0874464.
- ask reports a rounded usage-based CLI estimate of $0.0001.
- grep does not report complete usage/cost, so Jev total and combined cost are
  **unknown**, not zero and not the Claude amount alone. Cache/retries/failures
  cannot be priced from call counts or elapsed time.

Raw streams and journals remain local; no user transcripts or host metadata are
published. These first runs predate the additional binary identity fields now
captured by the runner; future treated runs record CLI version and binary hash.

## Conclusion

The integration works, but it is **not automatically used** on these survey tasks,
even in the inert-padding stress case. Do not claim survey improvement, cost
savings, or promote onboarding on this evidence. The fixtures remain tiny and
structurally easy to navigate; they do not establish behavior on real large repos.

Next: distinguish broad topology surveys from unknown-behavior investigations on
independently reviewed public repositories. Preserve freedom to choose cheap
Read/Grep rather than prescribing paid calls. Measure task success and combined
cost, not SessionKit call count. Complete grep usage telemetry is needed before
pricing runs that actually choose semantic search.

## Reproduce (explicit paid requests)

```sh
python3 eval/surveyor/run.py --variant survey --case tooling --tool-profile both
# Optional counterbalanced repetitions:
python3 eval/surveyor/run.py --variant survey --case tooling --tool-profile both --repeat 2
# Explicitly forced dispatch smoke, separate from adoption:
python3 eval/surveyor/run.py --variant survey --case development --tool-profile sessionkit --dispatch-check
```

Requires the current SessionKit CLI and a TypeSafe key for actual native calls.
`--sessionkit-binary` can select a trusted build. The Claude budget flag does not
cap Jev expenditure. Existing search caching and sensitive-source exclusions are
unchanged. The optional native tools send only eligible synthetic source to
TypeSafe; no comparison reads client repositories or mines conversation history.
