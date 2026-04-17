# OMC Phase 2 Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在现有 Phase 1 claw-native runtime 基础上，补齐 OMC 兼容层：契约对齐、生命周期 hooks、插件/slash command 兼容，以及外部 team runtime 兼容。

**Architecture:** 保持 `runtime` / `tools` / `commands` / `plugins` 作为 claw-native 核心，新增 OMC-facing adapter 和 bridge，而不是把核心实现改写成 OMC 原始形态。先做 2A/2B/2C，最后做 2D，确保每个阶段都有独立验证出口。

**Tech Stack:** Rust workspace (`runtime`, `tools`, `commands`, `plugins`, `rusty-claude-cli`), JSON state files under `.omx/`, existing `ModeStateStore`, `TaskRegistry`, `TeamRegistry`, `verification_runtime`, plugin manifest compatibility layer.

---

## 文件结构与责任划分

### 现有核心文件
- `rust/crates/runtime/src/mode_state.rs` — 统一模式状态持久化
- `rust/crates/runtime/src/deep_interview.rs` — deep-interview MVP
- `rust/crates/runtime/src/verification_runtime.rs` — verification substrate
- `rust/crates/runtime/src/task_registry.rs` — persisted task runtime
- `rust/crates/runtime/src/team_cron_registry.rs` — persisted team runtime
- `rust/crates/runtime/src/hooks.rs` — 当前运行时 hooks 实现
- `rust/crates/runtime/src/config.rs` / `config_validate.rs` — hooks 与 `mcpServers` 配置解析
- `rust/crates/plugins/src/lib.rs` / `hooks.rs` — 插件 manifest 契约与插件 hooks
- `rust/crates/commands/src/lib.rs` — slash command parsing / dispatch
- `rust/crates/rusty-claude-cli/src/main.rs` — CLI/REPL surface 与 OMC 兼容提示

### Phase 2 预计新增文件
- `rust/crates/runtime/src/omc_compat.rs` — OMC 契约桥接共用逻辑
- `rust/crates/runtime/src/omc_lifecycle.rs` — 新增 lifecycle hook state/event bridge
- `rust/crates/commands/src/omc_commands.rs` — `/oh-my-claudecode:*` 兼容解析与分发
- `rust/crates/plugins/src/omc_manifest_adapter.rs` — plugin manifest 字段兼容适配
- `rust/crates/runtime/src/external_team_runtime.rs` — `omc-teams` / tmux 外部 worker 兼容层（最后做）

> 原则：Phase 2 的新增逻辑优先放在 `omc_*` / `external_*` 新文件，不把 claw-native 核心模块做成耦合大杂烩。

## Task 1: Phase 2A — OMC 契约对齐层

**Files:**
- Create: `rust/crates/runtime/src/omc_compat.rs`
- Modify: `rust/crates/runtime/src/lib.rs`
- Modify: `rust/crates/runtime/src/mode_state.rs`
- Modify: `rust/crates/runtime/src/deep_interview.rs`
- Modify: `rust/crates/runtime/src/verification_runtime.rs`
- Modify: `rust/crates/runtime/src/team_cron_registry.rs`
- Test: `rust/crates/runtime/src/omc_compat.rs` (unit tests)

- [ ] **Step 1: 定义 OMC 兼容常量与字段映射**

```rust
pub const OMC_COMPAT_SCHEMA_VERSION: u32 = 1;

pub struct OmcCompatHandoff {
    pub handoff_path: String,
    pub next_skill: Option<String>,
    pub next_skill_args: Vec<String>,
}
```

- [ ] **Step 2: 在 `omc_compat.rs` 中实现 mode/handoff 正规化函数**

```rust
pub fn normalize_mode_name(mode: &str) -> &str;
pub fn build_omc_handoff(next_skill: Option<&str>, args: &[&str], path: &str) -> OmcCompatHandoff;
```

- [ ] **Step 3: 在 `lib.rs` 中导出新适配层 API**

Run: `cargo check -p runtime --quiet`
Expected: 成功，无未解析导出

- [ ] **Step 4: 让 deep-interview / verification / team 记录附带兼容 handoff 字段**

```rust
// 仅增加兼容字段，不改动现有核心字段含义
pub handoff: Option<OmcCompatHandoff>
```

- [ ] **Step 5: 增加单元测试覆盖 handoff 和 mode 正规化**

Run: `cargo test -p runtime omc_compat -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add rust/crates/runtime/src/omc_compat.rs rust/crates/runtime/src/lib.rs rust/crates/runtime/src/mode_state.rs rust/crates/runtime/src/deep_interview.rs rust/crates/runtime/src/verification_runtime.rs rust/crates/runtime/src/team_cron_registry.rs
git commit -m "Add an OMC contract adapter layer for Phase 2"
```

## Task 2: Phase 2B — 生命周期 Hooks 扩展

**Files:**
- Create: `rust/crates/runtime/src/omc_lifecycle.rs`
- Modify: `rust/crates/runtime/src/lib.rs`
- Modify: `rust/crates/runtime/src/hooks.rs`
- Modify: `rust/crates/runtime/src/config.rs`
- Modify: `rust/crates/runtime/src/config_validate.rs`
- Test: `rust/crates/runtime/src/omc_lifecycle.rs` (unit tests)

- [ ] **Step 1: 定义新增 hook event 枚举**

```rust
pub enum OmcLifecycleEvent {
    UserPromptSubmit,
    SessionStart,
    Stop,
}
```

- [ ] **Step 2: 在 `omc_lifecycle.rs` 中实现事件负载与 state bridge**

```rust
pub struct OmcLifecyclePayload {
    pub event: OmcLifecycleEvent,
    pub session_id: Option<String>,
    pub mode: Option<String>,
    pub message: Option<String>,
}
```

- [ ] **Step 3: 扩展 `config.rs` 支持这些 hooks 的配置解析，但保持旧行为兼容**

Run: `cargo test -p runtime config -- --nocapture`
Expected: 新旧 hooks 都能解析

- [ ] **Step 4: 在 `hooks.rs` 中把这些 lifecycle event 接入现有 hook runner**

- [ ] **Step 5: 针对 `UserPromptSubmit` / `SessionStart` / `Stop` 写单元测试**

Run: `cargo test -p runtime omc_lifecycle -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add rust/crates/runtime/src/omc_lifecycle.rs rust/crates/runtime/src/lib.rs rust/crates/runtime/src/hooks.rs rust/crates/runtime/src/config.rs rust/crates/runtime/src/config_validate.rs
git commit -m "Extend runtime hooks with OMC lifecycle events"
```

## Task 3: Phase 2C — 插件与 Slash Compatibility

**Files:**
- Create: `rust/crates/plugins/src/omc_manifest_adapter.rs`
- Create: `rust/crates/commands/src/omc_commands.rs`
- Modify: `rust/crates/plugins/src/lib.rs`
- Modify: `rust/crates/commands/src/lib.rs`
- Modify: `rust/crates/rusty-claude-cli/src/main.rs`
- Test: `rust/crates/plugins/src/omc_manifest_adapter.rs`
- Test: `rust/crates/commands/src/omc_commands.rs`

- [ ] **Step 1: 在 `omc_manifest_adapter.rs` 中实现对 `skills/agents/commands/mcpServers` 的兼容解析**

```rust
pub struct OmcManifestCompat {
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub commands: Vec<String>,
    pub mcp_servers: serde_json::Value,
}
```

- [ ] **Step 2: 让 `plugins/src/lib.rs` 在拒绝前先尝试兼容适配**

- [ ] **Step 3: 在 `omc_commands.rs` 中实现 `/oh-my-claudecode:*` 到现有技能/命令分发的映射**

```rust
pub fn parse_omc_slash_command(input: &str) -> Option<(String, Vec<String>)>;
```

- [ ] **Step 4: 在 `commands/src/lib.rs` 接入新解析逻辑**

- [ ] **Step 5: 在 CLI 主入口里去掉“does not yet load plugin slash commands”的兼容提示，改为真正分发**

Run: `cargo test -p commands omc_commands -- --nocapture`
Expected: `/oh-my-claudecode:deep-interview` 等可解析

- [ ] **Step 6: Commit**

```bash
git add rust/crates/plugins/src/omc_manifest_adapter.rs rust/crates/plugins/src/lib.rs rust/crates/commands/src/omc_commands.rs rust/crates/commands/src/lib.rs rust/crates/rusty-claude-cli/src/main.rs
git commit -m "Add OMC plugin and slash-command compatibility adapters"
```

## Task 4: Phase 2D — 外部 Team Runtime 兼容

**Files:**
- Create: `rust/crates/runtime/src/external_team_runtime.rs`
- Modify: `rust/crates/runtime/src/lib.rs`
- Modify: `rust/crates/runtime/src/team_cron_registry.rs`
- Modify: `rust/crates/tools/src/lib.rs`
- Test: `rust/crates/runtime/src/external_team_runtime.rs`

- [ ] **Step 1: 为 `omc-teams` / tmux worker 兼容定义最小外部 runtime 状态结构**

```rust
pub struct ExternalTeamWorker {
    pub worker_id: String,
    pub cli: String,
    pub pane_id: Option<String>,
    pub status: String,
}
```

- [ ] **Step 2: 把外部 worker 状态记录到现有 `.omx/state/team/...` 数据平面**

- [ ] **Step 3: 为外部 team runtime 提供最小启动/状态查询桥接 API**

- [ ] **Step 4: 将 team runtime 与外部 worker 兼容层连接，但不破坏当前 claw-native team MVP**

Run: `cargo test -p runtime external_team_runtime -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add rust/crates/runtime/src/external_team_runtime.rs rust/crates/runtime/src/lib.rs rust/crates/runtime/src/team_cron_registry.rs rust/crates/tools/src/lib.rs
git commit -m "Bridge the external OMC team runtime onto the persisted team substrate"
```

## 自检
- [ ] Phase 2A 没有重写 Phase 1 核心字段，只增加 compat adapter
- [ ] Phase 2B 仅先扩 `UserPromptSubmit` / `SessionStart` / `Stop`
- [ ] Phase 2C 真的让 `/oh-my-claudecode:*` 可分发，而不是继续只给错误提示
- [ ] Phase 2D 保持外部 runtime 是适配层，不反向污染核心 team runtime
- [ ] 所有步骤都给了明确文件路径、命令、预期结果，没有占位语句

## 执行交接
计划完成并建议保存到：
- `docs/superpowers/plans/2026-04-17-omc-phase-2-compatibility-implementation-plan.md`

推荐执行方式：
1. **Subagent-Driven（推荐）** — 用 `superpowers:subagent-driven-development`
2. **Inline Execution** — 用 `superpowers:executing-plans`
