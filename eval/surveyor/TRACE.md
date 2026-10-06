# Tracing and counterchecking

For each selected slice, follow entrypoint -> orchestration -> boundary -> store
or external interface. Cite implementations/call sites, not only imports or docs.
Do not execute the path just to describe it. Source indicates possible behavior,
not that a path ran or was deployed. Note dynamic dispatch and uninspected branches.

Before concluding what an operation does or cannot do, trace the values and
state it consumes, not just calls: is a job a copied payload or an identifier read
later? Which revision/cache key is used? Compare pending, completed and failure
states; follow acknowledgement/commit conditions. A missing new enqueue or explicit
cache invalidation does not by itself prove a missing future effect. Name the
preconditions and a counterexample to every consequential 'never/always' claim.
If those cannot be checked within scope, qualify the conclusion rather than guess.

At consequential boundaries, test the initial hypothesis:

- 'X owns writes': inspect direct persistence callers, administrative routes,
  workers and maintenance entrypoints. Report bypasses without deciding intent.
- 'Y is active': find a caller/registration/launch connection. Dependencies,
  class definitions and experimental folders alone do not prove deployment.
  Conversely, no direct import does not imply inactivity: inspect registries,
  configured import strings, factories and registration/dispatch paths.
- 'Config says Z': trace effective defaults, override order and declared launch
  environment. Distinguish a checked-in deployment declaration from the live state.
- 'Only one entrypoint': check package scripts and deployment/job declarations.
- 'The docs match': compare central claims against implementation and launch config.

State the inspected scope for negative results; absence in a search is not global
proof. Investigate contradictions before adding diagrams or more prose. Prioritize
missing central edges over exhaustive leaf inventories.
