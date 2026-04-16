# OMC Runtime Compatibility Design

## Metadata
- Date: 2026-04-16
- Status: Approved for planning
- Scope: `claw-code` Phase 1/Phase 2 support for OMC-inspired `deep-interview`, `ultrawork`, `ralph`, and `team`
- Strategy: Phase 1 behavior compatibility first, Phase 2 OMC contract compatibility

## Problem Statement
`claw-code` already has useful primitives—session persistence, task/team registries, worker boot state, skill lookup, and `AskUserQuestion`—but it does not yet have a unified orchestration substrate. The missing pieces are persistent mode state, persistent task/team runtime data, structured verification records, and a clean handoff path between clarification, execution, and coordination modes.

The goal is to add a claw-native orchestration runtime that can behave like core OMC workflows without immediately taking on full Claude Code plugin compatibility.

## Goals
1. Support behavior-compatible MVPs for `deep-interview`, `ultrawork`, `ralph`, and `team`.
2. Persist mode, task, team, and verification runtime state across interruptions.
3. Keep Phase 1 claw-native and upgradeable, so Phase 2 can add OMC-facing adapters without rewriting the core runtime.
4. Reuse existing `SessionStore`, `Session`, skill lookup, and worker boot machinery wherever possible.

## Non-Goals
- Full `/oh-my-claudecode:*` slash-command compatibility in Phase 1.
- Full Claude Code plugin manifest compatibility in Phase 1.
- Full OMC lifecycle hook coverage (`UserPromptSubmit`, `SessionStart`, `Stop`, `PreCompact`, etc.) in Phase 1.
- `omc-teams` tmux runtime or external `claude`/`codex`/`gemini` worker compatibility in Phase 1.
- Full OMC memory ecosystem (`notepad`, `project_memory`, `shared_memory`) in Phase 1.

## Existing Evidence
- `TaskRegistry` and `TeamRegistry` are currently in-memory only.
- `SessionStore` and `Session` already support workspace-scoped persistent session storage under `.claw/sessions/<workspace-hash>/` with per-session JSONL files.
- `worker_boot.rs` already implements a worker control-plane state machine.
- Skill resolution already scans `.omc`, `.agents`, `.claw`, `.codex`, and `.claude` roots.
- `claw` explicitly does not yet load OMC plugin slash commands, Claude statusline stdin, or OMC session hooks.
- Plugin manifest compatibility currently rejects Claude Code plugin contract fields such as `skills`, `agents`, `commands`, and `mcpServers`.

## Architecture
Phase 1 introduces a claw-native orchestration runtime with six modules:

1. **Mode State Store**
   - Unified runtime state for `deep-interview`, `ultrawork`, `ralph`, and `team`.
   - Tracks `active`, `current_phase`, `iteration`, `session_id`, timestamps, and runtime context.

2. **Persistent Task Store**
   - Replaces in-memory-only task lifecycle storage with durable task records.
   - Supports status, prompts, messages, output, dependencies, artifacts, and session/team linkage.

3. **Team Runtime**
   - Claude-native task grouping and phase orchestration.
   - Starts as a minimal coordination layer without tmux or external CLI workers.

4. **Verification Runtime**
   - Stores acceptance criteria, verifier outcomes, and command/test/build evidence.
   - Acts as the substrate for `ralph` completion logic.

5. **Session / Worker Bridge**
   - Reuses existing session persistence and worker boot control-plane code.
   - Connects orchestration state to resumable sessions rather than replacing session storage.

6. **Spec Artifact Layer**
   - Supports `deep-interview` output under `.omx/specs/`.
   - Provides a stable handoff artifact for later planning and execution modes.

### Dependency Shape
`Session / Worker Bridge -> Mode State Store -> {Task Runtime, Team Runtime, Verification Runtime}`

`deep-interview` feeds specs into later modes. `ultrawork` is the parallel execution layer. `ralph` depends on `ultrawork` plus verification and persistence. `team` depends on persistent task/team coordination.

## Data Model and File Layout
Use existing worktree conventions rather than inventing a new root.

### File Layout
```text
.omx/
  state/
    deep-interview-state.json
    ultrawork-state.json
    ralph-state.json
    team-state.json
    sessions/<session-id>/
      deep-interview-state.json
      ultrawork-state.json
      ralph-state.json
      team-state.json

  runtime/
    tasks/<task-id>.json
    teams/<team-id>.json
    verification/<run-id>.json
    indexes/tasks.json
    indexes/teams.json

  specs/
    deep-interview-<slug>.md

.claw/
  sessions/<workspace-hash>/<session-id>.jsonl
```

### Mode State Schema
```json
{
  "mode": "ralph",
  "active": true,
  "current_phase": "verification",
  "session_id": "session_abc123",
  "iteration": 2,
  "started_at": "2026-04-16T10:00:00Z",
  "updated_at": "2026-04-16T10:05:00Z",
  "completed_at": null,
  "context": {
    "current_task_ids": ["task_1", "task_2"],
    "team_id": "team_1",
    "verification_run_id": "verify_1"
  }
}
```

### Task Record Schema
```json
{
  "task_id": "task_x",
  "prompt": "Run parallel review and implementation tasks for auth hardening",
  "description": "Authentication hardening work item",
  "status": "running",
  "team_id": "team_1",
  "session_id": "session_abc123",
  "created_at": "2026-04-16T10:06:00Z",
  "updated_at": "2026-04-16T10:08:00Z",
  "messages": [],
  "output": "",
  "dependencies": [],
  "artifacts": []
}
```

### Team Record Schema
```json
{
  "team_id": "team_x",
  "name": "auth-fix",
  "status": "running",
  "phase": "team-exec",
  "session_id": "session_abc123",
  "task_ids": ["task_1", "task_2"],
  "created_at": "2026-04-16T10:07:00Z",
  "updated_at": "2026-04-16T10:09:00Z"
}
```

### Verification Record Schema
```json
{
  "verification_run_id": "verify_x",
  "session_id": "session_abc123",
  "mode": "ralph",
  "status": "pending",
  "acceptance_criteria": [
    {"id": "ac1", "text": "Workspace tests pass after auth hardening tasks complete", "passed": false}
  ],
  "checks": [
    {"kind": "build", "command": "cargo test --workspace", "status": "passed"}
  ],
  "reviewer": {
    "type": "verifier",
    "status": "approved"
  },
  "updated_at": "2026-04-16T10:15:00Z"
}
```

## Phase 1 MVP Scope
### deep-interview MVP
- One-question-at-a-time interview loop.
- Persistent interview state and ambiguity scoring.
- Spec output to `.omx/specs/deep-interview-<slug>.md`.
- Handoff into claw-native planning/execution lanes.

### ultrawork MVP
- Parallel task dispatch.
- Simple dependency grouping.
- Lightweight build/test verification.

### ralph MVP
- Iteration loop.
- Acceptance tracking.
- Verifier gate.
- Resume/retry support.

### team MVP
- Create team.
- Assign tasks.
- Track phase transitions.
- Aggregate status.

## Implementation Order
1. Mode State Store
2. Persistent Task Store
3. deep-interview MVP
4. ultrawork MVP
5. Verification Runtime
6. ralph MVP
7. team MVP

This order gives the earliest usable clarification flow after the shared state layer lands, then layers execution, verification, persistence, and coordination without forcing later rewrites.

## Risks and Mitigations
### Risk 1: Embedding mode logic directly into current registries
Mitigation: keep registries/data-access thin; implement orchestration behavior in a new runtime layer.

### Risk 2: Chasing OMC surface compatibility too early
Mitigation: Phase 1 targets behavior compatibility only.

### Risk 3: Shipping `ralph` before structured verification exists
Mitigation: require verification runtime before `ralph` MVP completion.

### Risk 4: Implementing tmux/external worker team runtime too early
Mitigation: keep Phase 1 `team` Claude-native only.

## Milestones
### Milestone 1
- Mode state persistence
- Persistent task store

### Milestone 2
- deep-interview MVP
- ultrawork MVP

### Milestone 3
- verification runtime
- ralph MVP

### Milestone 4
- team MVP

## Phase 2 Upgrade Path
Phase 2 adds OMC-facing adapters without replacing the Phase 1 core.

### Phase 2A: Contract Alignment
- Align state/handoff field semantics with OMC expectations.
- Keep claw-native core, add stable adapter boundaries.

### Phase 2B: Lifecycle Hooks
- Add `UserPromptSubmit`, `SessionStart`, and `Stop` coverage first.
- Defer `SubagentStart/Stop`, `PreCompact`, and full lifecycle parity until later.

### Phase 2C: Plugin and Slash Compatibility
- Add adapter support for `/oh-my-claudecode:*` style slash commands.
- Bridge plugin-managed `skills`, `agents`, `commands`, and `mcpServers` where feasible.

### Phase 2D: External Team Runtime
- Add `omc-teams` / tmux / external CLI worker compatibility.
- Build this last, after team runtime, state semantics, and lifecycle hooks stabilize.

## Design Principle
The core runtime should remain claw-native:

```text
core runtime = claw-native orchestration substrate
compat layer = OMC-facing adapters and lifecycle bridges
```

That separation preserves maintainability and minimizes rewrite risk while still creating a realistic path to stronger OMC compatibility.

## Acceptance Criteria
- A planning artifact exists that defines a claw-native runtime for `deep-interview`, `ultrawork`, `ralph`, and `team`.
- The design defines explicit Phase 1 non-goals to prevent scope drift.
- The design defines persistent file layouts and schemas for mode, task, team, and verification data.
- The design defines an implementation order that makes `deep-interview` and `ultrawork` possible before `ralph` and `team` complete.
- The design defines a Phase 2 path that adds OMC compatibility without replacing the Phase 1 core.
