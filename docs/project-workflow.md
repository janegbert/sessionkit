# Compact project workflow (experimental)

Opt-in coaching for improving an agent's approach without accumulating narrow
skills or memories. It adds one stable instruction section to the main agent
and guidance to subagent prompts. No extra Jev calls, transcript analysis,
filesystem writes or helper execution occur in the hooks.

## Enable and use

```sh
printf '%s\n' '{"projectWorkflow":"true"}' |
  claude plugin configure sessionkit@sessionkit --values-stdin
```

Restart Claude Code. The option is off by default. Disable its automatic coaching
with `"projectWorkflow":"false"` and restart again.

From a project directory, explicitly invoke:

```text
/sessionkit:project-workflow onboard
/sessionkit:project-workflow review
/sessionkit:project-workflow propose <verified recurring lesson>
```

These commands request proposals, not blanket approval to write. They remain
available explicitly when coaching is off. Onboard starts from existing project
skills and canonical documentation. Review favors pruning and consolidation.
Propose usually retains nothing unless the lesson is durable, verified and
recurring, not already covered or cheaply derivable from source.

## One entrypoint, selective depth

Reuse a compatible existing project skill. When a new Claude Code entrypoint is
needed, the default location is:

```text
.claude/skills/project-workflow/
├── SKILL.md                    # compact routing and maintenance policy
├── references/                 # only if existing docs cannot serve the need
│   └── <focused-workflow>.md
└── scripts/                    # only if existing repo tooling is insufficient
    └── <bounded-inspection-helper>
```

Start with SKILL.md alone where possible. Link to existing CONTRIBUTING, test or
configuration docs instead of copying them. Load only the reference matching the
current task. Project facts belong in source/configuration, not duplicate memories.
An unrelated domain skill need not be merged just to achieve a one-skill count.

Review defaults: <=60 entrypoint lines, <=3 owned references and <=2 focused
helpers. Inspect the entire owned package; moving clutter to subfiles is not a
solution. These limits are coaching/review budgets, not enforced byte/line caps.
Exceptions require a justification and user approval. No helper is generated just
to fill a directory; no script runs because its reference was loaded.

## Approval and boundaries

The agent first adapts within its current task. A persistence proposal includes
all exact diffs and destinations, supporting evidence, applicability and limits.
It waits for explicit user acceptance, rechecks that the patch is current, applies
only accepted changes and verifies links and budgets. Subagents return proposals
to the parent and do not persist workflow knowledge themselves.

**This is instruction-based coaching, not a memory firewall.** It does not block
Claude Code's native memory tools, other plugins, architecture decision recording
or the independent `sessionkit skills --draft` command. Nor does disabling the
option unload a project skill already created and discovered by Claude Code.
Review or remove that project artifact explicitly if it is no longer wanted.
Ordinary permission/organization ceilings remain in force. Linked scripts and
documentation never confer permission to execute, install or make paid requests.

Project files still enter the agent's context through normal reads. This feature
adds no external service of its own; it does not make the host model local or
change the disclosure/cost rules of other tools an agent chooses to use.

## Implementation and verification

- `plugin/hooks/project-workflow.js`: opt-in, stable/deduplicated prompt coaching
  and subagent guidance; no side-effect APIs.
- `plugin/skills/project-workflow/`: explicit onboarding/review/proposal procedures
  and a drafting template, lazily read rather than injected in every prompt.
- Embedded in `sessionkit setup`, including all linked plugin reference files.
- Hook tests cover opt-out/default, deduplication, preserved metadata, subagent
  propagation, policy/budget language and error propagation. An API-access trap
  fails if the tested hooks attempt filesystem/process/model or other operations.

A live Claude Code 2.1.291 Sonnet check invoked the installed review command on
an artificial project at `/tmp/sessionkit-workflow-smoke.SBlYQn/fixture`. It found
an overlapping route, broken reference, per-success memory instruction and a
helper that wrote state. It returned an entrypoint diff, suggested deleting the
helper and asked for acceptance. File hashes stayed unchanged and no helper state
was created. The check offered only Read/Glob/Grep/Skill, so it did not exercise
accepted writes or prove voluntary restraint when write tools are available.
The model's post-edit line estimate was inaccurate, and the helper deletion was
described rather than supplied as a full deletion diff: review budgets and exact
patch compliance are not yet reliably enforced. Output is kept locally beside
the fixture as `output.jsonl`; no transcript is included in the repository.

Tests verify registration and instruction delivery, not consistent agent
compliance or a measured reduction in memory/skill proliferation. A behavioral evaluation should
include one-off lessons (no retention), recurring verified workflows (small edit),
existing overlapping skills (reuse), stale approval (re-propose), unsafe helpers
(no execution) and subagent suggestions (proposal only). Count the resulting
owned files and context overhead, not just the number of SKILL.md entrypoints.
