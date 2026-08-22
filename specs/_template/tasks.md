# Tasks: {{NNN_SLUG}}

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## Tasks

### 1. [Task name]
- Files or areas: `[path]`, `[path]`
- Change: [Concrete implementation action]
- Verification:
  - [Exact functional or integration test]
  - [Exact regression check]
- Done when:
  - [Observable result]

### 2. [Task name]
- Files or areas: `[path]`, `[path]`
- Change: [Concrete implementation action]
- Verification:
  - [Exact functional or integration test]
  - [Exact regression check]
- Done when:
  - [Observable result]

## Final Verification

Before closing the packet, run:

- [Build or typecheck command]
- [Relevant test command]
- [Most representative real functionality command or scenario]
