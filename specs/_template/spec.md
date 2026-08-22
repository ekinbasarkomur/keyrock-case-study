---
spec_name: "{{FULL_SPEC_NAME}}"
spec_id: "{{SPEC_ID}}"
spec_folder: "{{NNN_SLUG}}"
status: "draft"
created_at: "YYYY-MM-DD"
updated_at: "YYYY-MM-DD"
created_by: "{{CREATED_BY}}"
creation_mode: "{{CREATION_MODE}}"
source_inputs:
  - "inputs/human.md"
source_agents: []
goal: "[Short outcome statement]"
purpose: "[Why this work exists]"
parent_request: "[Issue, ticket, prompt, or brief reference]"
related_paths:
  - "[path/to/primary/area]"
verification_level: "mixed"
complexity: "small"
---

# Spec: {{NNN_SLUG}}

## Problem

[What problem exists now]

## Goal

[What should be true after this work]

## Purpose

[Why this work matters now]

## Out of Scope

- [Explicit non-goal]
- [Explicit non-goal]

## Current State

[Verified repository facts, affected modules, relevant behavior, and constraints]

## Proposed Design

[Describe the intended behavior and technical shape without turning this into a task list]

## Acceptance Criteria

- [Observable user or system behavior]
- [Observable user or system behavior]
- [Operational or quality criterion]

## Invariants and Critical Don'ts

- [Constraint that must remain true]
- [What must not be broken or changed]

## Risks and Tradeoffs

- [Risk]
- [Tradeoff]

## Testing Strategy

Required real verification:

- [Integration or functional path]
- [Regression test]
- [Broader verification command if relevant]

Optional supporting checks:

- [Unit test or static check]

## Rollback Plan

[How to back out safely if the change causes issues]

## Open Questions

- [Question]
