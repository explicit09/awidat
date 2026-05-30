---
name: {{name}}
description: {{description}}
version: "0.1.0"
awidat_min_version: "0.1.0"
when_to_use: |
  Activate when the user wants X or when the project type is Y.
---

# {{name}}

Brief overview of what this skill does and when it should run.

## Editorial principles

- Bullet list of decisions this skill makes
- Each one short and clear

## Tools you'll use

- `apply_edl` — for cut/trim/delete proposals
- `use_generated_media` — for B-roll insertions
- Add tools as needed

## Rationale rules

Every proposal you emit MUST include a `rationale` field per Awidat's
contract (see L1 catalog). Example: `"Trimmed 0.4s — silence > 300ms
threshold from AGENTS.md podcast defaults."`

## When NOT to act

- If the project type doesn't match (check AGENTS.md)
- If the requested edit conflicts with user must-keep marks
- If the user said "leave it alone" anywhere in conversation
