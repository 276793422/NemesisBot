---
name: skill-creator
description: Create or update skills for this agent (SKILL.md plus optional bundled resources). Use when the user asks to create, write, improve, or restructure a skill.
---

# Skill Creator

How to author a skill that this agent's skill system loads and follows.

## What a skill is

A skill is a directory containing a required `SKILL.md`. The loader reads the
YAML frontmatter — only `name` and `description` are consumed — and loads the
Markdown body on demand after the skill triggers. Resolution order (higher
overrides lower by name):

1. Workspace skills (`<workspace>/skills/`)
2. Global skills (`~/.nemesisbot/skills/`)
3. Built-in skills (embedded, shipped with the binary)

The `description` is the only trigger mechanism: it is always in context while
the body is not. Put all "when to use" information there — a "When to Use"
section in the body is useless for triggering.

## Layout

```
my-skill/
├── SKILL.md            # required: frontmatter (name, description) + body
├── scripts/            # optional: executable helpers for deterministic steps
├── references/         # optional: docs loaded into context only when needed
└── assets/             # optional: files used in output, never loaded
```

## Writing guidelines

- **Name**: lowercase letters, digits, hyphens; folder name matches the name.
- **Description**: state what the skill does AND the trigger phrases/contexts.
  Keep it under a few hundred characters.
- **Body**: concise, imperative, under ~200 lines. Assume the agent is already
  capable — include only what is non-obvious: exact commands, file formats,
  pitfalls, project-specific rules.
- **Progressive disclosure**: keep the body lean; move large reference material
  into `references/` and link it from the body with a note on when to read it.
  One level deep — reference files should not chain further references.
- **No filler files**: no README, CHANGELOG, or installation guides. The skill
  is read by an agent, not a human installing software.
- **Scripts**: only when the same code would be rewritten repeatedly or
  determinism matters. Test any script you add by actually running it.

## Managing skills at runtime

- Tools: `skills_list`, `skills_info`, `find_skills`, `install_skill`,
  `skill_manage` (enable/disable/remove).
- Remote registries: configured in `<workspace>/config/config.skills.json`;
  search and install via the skills tools.
- Distilling experience: after completing a non-trivial task, capture the
  reusable procedure as a new skill instead of letting it live only in the
  conversation.

## Worked examples in this repository

Read these for tone and length calibration:

- `skills/weather/` — minimal single-purpose reference skill.
- `skills/cluster/` — multi-step operational workflow.
- `skills/github/` — tool-integration skill around an external CLI.
