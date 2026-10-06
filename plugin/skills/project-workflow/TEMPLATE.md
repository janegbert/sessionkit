# Entrypoint shape

This is a drafting example, not an installed project skill. Replace placeholders
with verified project facts, omit unnecessary routes, and do not create the
referenced files just because they appear here. Keep the resulting SKILL.md at
most 60 lines including frontmatter. Use the existing skill's name when updating.

```markdown
---
name: project-workflow
description: <Specific workflows this project skill covers and when to use it.>
---

# <Project> workflow

Use this skill for <verified recurring project operations>. It is a route map,
not a replacement for repository instructions or permission checks.

## Routes

- <When inspecting script/config execution>: read <verified relative link to
  existing configuration docs>. Trace the entrypoint and effective overrides;
  retrieve current values from source instead of storing them here.
- <When selecting tests>: read <verified relative link to existing test docs>.
  Choose the smallest relevant suite; broaden when the change crosses boundaries.
- <Only if a third documented workflow is useful>: read <one matching reference>.

Load only the route needed for the task. Inspect a helper before considering its
execution; a link grants no permission to run it.

## Maintenance

Adapt within the task first. Retain only verified, durable recurring procedures.
Prefer fixing canonical docs or improving/pruning this package to another skill,
reference or memory. Propose the exact diff and wait for explicit user approval.
```

From the default `.claude/skills/project-workflow/SKILL.md`, a link to the root
CONTRIBUTING.md would be `../../../CONTRIBUTING.md`, but include it only if that
file actually exists. A link from an owned `references/*.md` file needs another
`../`. Verify targets at the chosen location rather than copying these examples.
