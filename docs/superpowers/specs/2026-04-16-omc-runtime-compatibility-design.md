# OMC 运行时兼容性设计

## 元数据
- 日期：2026-04-16
- 状态：已批准，可进入规划阶段
- 范围：为 `claw-code` 设计分阶段支持 OMC 风格的 `deep-interview`、`ultrawork`、`ralph` 与 `team`
- 策略：Phase 1 先做行为兼容，Phase 2 再做 OMC 契约兼容

## 问题陈述
`claw-code` 已经具备一些有价值的基础能力——会话持久化、任务/团队注册表、worker 启动状态机、skill 查找与 `AskUserQuestion`——但还缺少统一的编排底座。当前缺口主要在于：模式状态持久化、任务/团队运行时数据持久化、结构化验证记录，以及澄清模式、执行模式、协调模式之间的稳定交接路径。

目标是在不立即承担完整 Claude Code 插件兼容成本的前提下，增加一个 claw-native 的编排运行时，使其能够表现出 OMC 核心工作流的主要行为。

## 目标
1. 提供 `deep-interview`、`ultrawork`、`ralph` 与 `team` 的行为兼容 MVP。
2. 让模式、任务、团队与验证运行时状态能够跨中断持久化。
3. 让 Phase 1 保持 claw-native 且可升级，使 Phase 2 能在不重写核心运行时的情况下增加 OMC 适配层。
4. 尽可能复用现有的 `SessionStore`、`Session`、skill 查找逻辑与 worker boot 机制。

## 非目标
- Phase 1 不做完整 `/oh-my-claudecode:*` slash command 兼容。
- Phase 1 不做完整 Claude Code 插件 manifest 兼容。
- Phase 1 不做完整 OMC 生命周期 hooks 覆盖（如 `UserPromptSubmit`、`SessionStart`、`Stop`、`PreCompact` 等）。
- Phase 1 不做 `omc-teams` 的 tmux 运行时，也不兼容外部 `claude` / `codex` / `gemini` worker。
- Phase 1 不做完整 OMC memory 生态（`notepad`、`project_memory`、`shared_memory`）。

## 现有证据
- `TaskRegistry` 与 `TeamRegistry` 当前都是纯内存实现。
- `SessionStore` 与 `Session` 已支持基于工作区的持久化，会话文件位于 `.claw/sessions/<workspace-hash>/` 下。
- `worker_boot.rs` 已实现 worker 控制平面的状态机。
- skill 解析已经会扫描 `.omc`、`.agents`、`.claw`、`.codex` 与 `.claude` 根路径。
- `claw` 明确声明当前尚不加载 OMC 插件 slash commands、Claude statusline stdin 或 OMC session hooks。
- 插件 manifest 兼容层目前会拒绝 Claude Code 插件契约中的 `skills`、`agents`、`commands` 与 `mcpServers` 等字段。

## 架构
Phase 1 引入一个 claw-native 的编排运行时，由六个模块构成：

1. **Mode State Store**
   - 为 `deep-interview`、`ultrawork`、`ralph` 与 `team` 提供统一运行态。
   - 记录 `active`、`current_phase`、`iteration`、`session_id`、时间戳与运行时上下文。

2. **Persistent Task Store**
   - 将仅存在于内存中的任务生命周期数据改为可持久化任务记录。
   - 支持状态、提示词、消息、输出、依赖、产物，以及 session/team 关联。

3. **Team Runtime**
   - 提供 claw-native 的任务分组与阶段化协同。
   - 初期只做最小协调层，不接入 tmux 或外部 CLI workers。

4. **Verification Runtime**
   - 保存验收标准、verifier 结果，以及命令/测试/构建证据。
   - 作为 `ralph` 完成判定的基础设施。

5. **Session / Worker Bridge**
   - 复用现有会话持久化与 worker boot 控制平面代码。
   - 将编排状态连接到可恢复会话，而不是替换现有会话存储。

6. **Spec Artifact Layer**
   - 支持 `deep-interview` 在 `.omx/specs/` 下输出 spec。
   - 为后续 planning 与 execution 模式提供稳定的交接产物。

### 依赖关系
`Session / Worker Bridge -> Mode State Store -> {Task Runtime, Team Runtime, Verification Runtime}`

`deep-interview` 负责生成 spec 并交给后续模式。`ultrawork` 是并行执行层。`ralph` 依赖 `ultrawork`，同时依赖验证与持久化。`team` 依赖持久化的任务/团队协调能力。

## 数据模型与文件布局
优先复用现有工作区约定，而不是新造一个根目录。

### 文件布局
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

## Phase 1 MVP 范围
### deep-interview MVP
- 单轮单问题的访谈循环。
- 可持久化的访谈状态与 ambiguity score。
- 输出 spec 到 `.omx/specs/deep-interview-<slug>.md`。
- 能交接到 claw-native 的 planning / execution 流程。

### ultrawork MVP
- 并行任务分发。
- 简单依赖分组。
- 轻量 build/test 验证。

### ralph MVP
- iteration loop。
- acceptance tracking。
- verifier gate。
- resume / retry 能力。

### team MVP
- 创建 team。
- 分配 tasks。
- 跟踪 phase transitions。
- 聚合状态展示。

## 实现顺序
1. Mode State Store
2. Persistent Task Store
3. deep-interview MVP
4. ultrawork MVP
5. Verification Runtime
6. ralph MVP
7. team MVP

这个顺序可以在共享状态层落地后，尽快得到第一个可用的澄清模式，然后逐步叠加执行、验证、持久化与协调能力，同时避免后续推倒重来。

## 风险与缓解
### 风险 1：把模式逻辑直接塞进当前 registries
缓解：保持 registry / data access 层足够薄，把编排行为放进新的 runtime 层。

### 风险 2：过早追求 OMC 表面兼容
缓解：Phase 1 仅追求行为兼容。

### 风险 3：在结构化验证能力未落地前就上线 `ralph`
缓解：要求在 `ralph` MVP 完成前先落地 verification runtime。

### 风险 4：过早做 tmux / 外部 worker team runtime
缓解：Phase 1 的 `team` 仅限 claw-native 协调模式。

## 里程碑
### Milestone 1
- mode state 持久化
- persistent task store

### Milestone 2
- deep-interview MVP
- ultrawork MVP

### Milestone 3
- verification runtime
- ralph MVP

### Milestone 4
- team MVP

## Phase 2 升级路径
Phase 2 在不替换 Phase 1 核心的前提下，增加面向 OMC 的适配层。

### Phase 2A：契约对齐
- 让 state / handoff 字段语义尽量与 OMC 对齐。
- 保持 claw-native 核心，实现稳定 adapter 边界。

### Phase 2B：生命周期 Hooks
- 优先补 `UserPromptSubmit`、`SessionStart` 与 `Stop`。
- `SubagentStart/Stop`、`PreCompact` 与更完整的生命周期对齐后续再做。

### Phase 2C：插件与 Slash Compatibility
- 增加 `/oh-my-claudecode:*` 风格 slash command 的适配支持。
- 在可行范围内桥接插件管理的 `skills`、`agents`、`commands` 与 `mcpServers`。

### Phase 2D：外部 Team Runtime
- 增加 `omc-teams` / tmux / 外部 CLI worker 兼容。
- 这一步最后做，前提是 team runtime、state 语义与 lifecycle hooks 都已稳定。

## 设计原则
核心运行时应保持 claw-native：

```text
core runtime = claw-native orchestration substrate
compat layer = OMC-facing adapters and lifecycle bridges
```

这种分层能在保持可维护性的同时，降低重写风险，并为后续更强的 OMC 兼容性留出清晰升级路径。

## 验收标准
- 存在一份规划产物，定义了 `deep-interview`、`ultrawork`、`ralph` 与 `team` 的 claw-native 运行时设计。
- 设计明确列出 Phase 1 非目标，防止范围失控。
- 设计定义了 mode、task、team 与 verification 数据的持久化布局与 schema。
- 设计定义了一个实现顺序，使 `deep-interview` 与 `ultrawork` 能在 `ralph` 与 `team` 之前落地。
- 设计定义了 Phase 2 路径，能够在不替换 Phase 1 核心的前提下增加 OMC 兼容性。
