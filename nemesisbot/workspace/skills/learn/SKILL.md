---
name: learn
description: Distill a directory, URL, conversation, or notes into a reusable skill. Use when the user wants to capture a workflow or reference as a slash command.
---

# Learn — distill a source into a reusable skill

Turn something the user points at — a local directory, a documentation URL, a workflow
from the current conversation, or pasted notes — into a reusable `SKILL.md` persisted under
`workspace/skills/<name>/`, so it becomes a slash command the agent can invoke later. This
is the agent's way of building procedural memory from existing material.

## When to use

Triggered by requests like:

- "learn the REST client in ~/projects/acme-sdk, focus on auth + pagination"
- "learn https://docs.example.com/api/quickstart"
- "learn how I just deployed the staging server"
- "learn filing an expense: open the portal, New > Expense, attach the receipt, submit"

## Procedure

1. **Identify the source kind** and gather material with the tools you already have:
   - Local directory / file → `read_file` / `list_dir` (read the README, entry points, and the
     modules most relevant to what the user asked about).
   - Online documentation URL → `web_fetch`.
   - Current conversation → use the workflow the user just walked you through.
   - Pasted notes / described procedure → the user's text itself.

   Read enough to understand the procedure end to end — not every line of the source.

2. **Extract the reusable knowledge**: the goal, the concrete ordered steps, the
   commands / APIs / files involved, the common pitfalls, and how to confirm success.
   Discard one-off detail that will not generalize.

3. **Author a `SKILL.md`** following the house standard (see the `skill-creator` skill for the
   full guide). Required shape:

   - YAML frontmatter with `name` (lowercase letters, digits, hyphens only, <64 chars) and
     `description` (short, and it **must state both what the skill does and when to use it** —
     the description is the trigger).
   - Body in this order: `# Title`, `## When to Use`, `## Procedure` (numbered steps),
     `## Pitfalls`, `## Verification`.
   - Frame steps using NemesisBot's real tools (`read_file`, `web_fetch`, `exec`, `edit_file`,
     `cluster_rpc`, …). **Never invent commands that do not exist.**

4. **Persist it** by calling the `skill_manage` tool exactly once:

   ```json
   {"action":"create","name":"<slug>","content":"<full SKILL.md incl. --- frontmatter --- >"}
   ```

   The tool security-checks the content and refuses to write anything dangerous. If it reports
   the skill already exists, ask the user whether to set `overwrite: true` or pick a new name —
   do not silently clobber.

5. **Report back**: tell the user the new slash-command name and a one-line summary. The skill
   is now live and visible via `skills_list` / `skills_info` (no restart needed).

## Pitfalls

- A weak `description` with no "when to use" clause means the skill never triggers. Always
  include the trigger condition in the frontmatter description.
- Don't paste the entire source verbatim — distill to the reusable procedure. Keep `SKILL.md`
  lean; if deep reference material is genuinely needed, write it to a `references/<file>.md`
  via `skill_manage` `write_file` and link to it from `SKILL.md`.
- The name must be a valid slug. Normalize titles like "Deploy Staging" to `deploy-staging`.

## Verification

After persisting, call `skills_info` with the new name and confirm it returns the content with
valid frontmatter and an acceptable lint score. Summarize what the new skill does for the user.
