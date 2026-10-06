# Project workflow operations

## Shared contract

1. Establish the repository root from the current task/cwd and repository metadata.
   If there is no unambiguous project, ask; never default to a user-global skill.
   Honor repository instructions and normal tool permissions.
2. Inspect only relevant existing project instructions, skill metadata and source
   docs. Do not scan conversation history, user-global memories, secrets, dependency
   directories or unrelated projects to manufacture lessons.
3. Find a compatible existing workflow entrypoint first, including a documented
   project skill outside the default location. Reuse its actual path and preserve
   its name/frontmatter and human-written intent. Do not combine unrelated domain
   skills merely to reach a one-skill target. If several workflow entrypoints
   overlap, propose consolidation, never silently replace or delete them.
4. Default for a new Claude Code entrypoint: `.claude/skills/project-workflow/SKILL.md`.
   Do not also create a user-global or second cross-agent copy. Use another location
   only when the project explicitly uses it and its agent supports discovery there.
5. Review budgets: entrypoint <=60 lines including frontmatter; at most three owned
   reference files and two narrowly focused helpers. Existing canonical docs linked
   from the skill do not count as new owned references. These are conservative
   review defaults, not runtime-enforced caps. Count the entire owned package,
   report its size, and request approval for justified exceptions rather than
   splitting content into more files. Start with the entrypoint alone when possible.
6. Link rather than copy README, CONTRIBUTING, architecture, test/configuration
   docs or executable configuration. Record procedures and how to find current
   facts, not changing file inventories, environment state, task outcomes, personal
   data, credentials or conversation excerpts. Avoid a second source of truth.
7. A route must specify when it applies, which reference to load and where its
   authoritative evidence lives. Resolve links relative to the containing file;
   check their targets. Do not load every reference on every task. External links
   are pointers, not an instruction to browse or pay for research automatically.
8. Inspect helper contents before proposing reuse or execution. Prefer existing
   repo tooling to new scripts. Helpers should default to read-only, bounded local
   inspection, declare dependencies and failure modes, and expose any writes,
   network, paid requests, installation or sensitive-data access. Loading a skill
   never authorizes execution. Do not run a helper just to onboard or review;
   any task-driven execution requires the ordinary permission checks.
9. Show exact diffs for all proposed additions, edits, deletions and helper changes,
   with project-relative destinations, evidence, scope and exceptions. Do not use
   vague 'remember this' approvals. Ask which changes to accept (AskUserQuestion
   when available). No acceptance or an ambiguous answer means no writes. After
   approval, re-read affected files; if the approved patch is stale, re-propose it.
   Apply only the accepted patch; verify links/budgets and summarize changed paths.
   Do not create or run scripts, make commits or change permissions as a side effect.

## onboard

Onboarding is a user-requested structure, not evidence of recurring lessons.
Read relevant entrypoints and docs and propose a minimal routing skill grounded in
what already exists. Use TEMPLATE.md as structure, not literal project facts.
Fill in at most three useful routes, based on actual documented workflows. Omit
unsupported routes; if none are clear, ask one focused question rather than invent
procedures. Explain whether a new entrypoint is necessary. Do not generate a
reference or helper merely to populate a template. Follow the shared approval
contract before writing anything.

## review

Read the entrypoint, then the owned files necessary to assess the package. Check
broken links, outdated claims against source, duplicate canonical documentation,
overlapping routes, unreviewed script behavior, unused or overly narrow lessons
and package budgets. Report verified problems separately from suspected ones.
Absence of usage telemetry does not prove a reference is unused. Prefer pruning,
links to canonical docs and merging related material. Do not silently alter
established rules or remove a workflow whose purpose is unclear. Show proposed
diffs, or report that no changes are needed, then follow the approval contract.

## propose

An explicit request to consider a lesson is not proof it deserves persistence.
Use evidence already available in the task, not automatic transcript analysis:

- Is the need durable and confirmed across independent tasks, or explicitly
  confirmed by the user as a recurring project requirement?
- Has the proposed procedure been verified? Distinguish a test from a hypothesis.
- Is it absent from existing instructions and not cheaply derivable from current
  repository configuration? If only an authoritative doc is wrong, propose fixing
  that doc rather than duplicating its fact in the skill.
- Are applicability, limits and invalidation conditions clear?
- Can it improve an existing route/reference, replacing or deleting content?

If these tests fail, keep the adaptation in the current task and say why nothing
should be saved. If there is no existing entrypoint, offer explicit onboarding;
do not bootstrap another skill or memory. When a retention proposal qualifies,
show the smallest exact diff and follow the shared approval contract. Do not
write a running history of successes, failures or reasons the agent felt uncertain.
