# Tracing and counterchecking

For each selected slice, follow entrypoint -> orchestration -> boundary -> store
or external interface. Cite implementations/call sites, not only imports or docs.
Do not execute the path just to describe it. Source indicates possible behavior,
not that a path ran or was deployed. Note dynamic dispatch and uninspected branches.

At consequential boundaries, test the initial hypothesis:

- 'X owns writes': inspect direct persistence callers, administrative routes,
  workers and maintenance entrypoints. Report bypasses without deciding intent.
- 'Y is active': find a caller/registration/launch connection. Dependencies,
  class definitions and experimental folders alone do not prove deployment.
- 'Config says Z': trace effective defaults, override order and declared launch
  environment. Distinguish a checked-in deployment declaration from the live state.
- 'Only one entrypoint': check package scripts and deployment/job declarations.
- 'The docs match': compare central claims against implementation and launch config.

State the inspected scope for negative results; absence in a search is not global
proof. Investigate contradictions before adding diagrams or more prose. Prioritize
missing central edges over exhaustive leaf inventories.
