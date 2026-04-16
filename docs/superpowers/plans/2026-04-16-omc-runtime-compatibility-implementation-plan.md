# OMC 运行时兼容性实现计划

## 元数据
- 日期：2026-04-16
- 关联规格：`docs/superpowers/specs/2026-04-16-omc-runtime-compatibility-design.md`
- 状态：待实现
- 范围：Phase 1 行为兼容实现 + 为 Phase 2 适配层预留接口

## 计划目标
按既定规格，把 `claw-code` 扩展为支持以下四个核心模式的 claw-native 运行时：
- `deep-interview`
- `ultrawork`
- `ralph`
- `team`

本计划只覆盖 **Phase 1 行为兼容实现**，不实现完整 OMC 插件、slash command、tmux 外部 worker 或全量 lifecycle hooks。

## 实施原则
1. 先补共享底座，再接模式。
2. 数据持久化与模式逻辑分层，避免把编排逻辑直接塞进现有 registry。
3. 优先复用 `SessionStore`、`Session`、`worker_boot`、现有 skill 解析逻辑。
4. 所有新能力都要有最小验证路径与可回滚边界。

## 里程碑总览

### Milestone 1：Mode State Store + Persistent Task Store
**目标**：提供后续四模式共用的持久化运行时底座。

#### 任务
1. 设计并实现统一 mode state schema。
2. 在 `.omx/state/` 与 `.omx/state/sessions/<session-id>/` 下落地状态读写逻辑。
3. 将当前 `TaskRegistry` 升级为可持久化 task store。
4. 为 task 增加 `session_id`、`dependencies`、`artifacts` 等字段。
5. 补充 task 索引文件（如 `tasks.json`）与恢复逻辑。

#### 交付物
- mode state 读写能力
- persistent task runtime
- 迁移后的 task 查询/更新接口

#### 验证
- 能创建、更新、恢复 mode state
- 任务在进程重启后仍可读取
- 现有 task tool surface 不被破坏

---

### Milestone 2：deep-interview MVP + ultrawork MVP
**目标**：先落地一个澄清模式和一个执行模式，验证底座可用。

#### deep-interview 任务
1. 定义 interview state 数据结构（轮次、ambiguity、phase、handoff spec path）。
2. 复用 `AskUserQuestion` 实现单轮提问。
3. 将访谈状态持久化到 mode state。
4. 输出 spec 到 `.omx/specs/deep-interview-<slug>.md`。
5. 定义 handoff metadata，供 plan / ralph / team 后续接入。

#### ultrawork 任务
1. 定义 parallel task grouping 与依赖分组策略。
2. 在 persistent task store 之上实现并发任务调度。
3. 增加轻量验证记录（build/test 结果摘要）。
4. 为 ultrawork 模式写入/恢复状态。

#### 交付物
- deep-interview MVP
- ultrawork MVP
- 规格产物与执行任务之间的首条可验证链路

#### 验证
- deep-interview 能跨中断恢复并重新继续提问
- ultrawork 能并发发射任务并收敛结果
- 产出的 spec 能作为后续模式输入

---

### Milestone 3：Verification Runtime + ralph MVP
**目标**：补齐完成判定基础设施，再实现持续执行模式。

#### Verification Runtime 任务
1. 定义 verification record schema。
2. 记录 acceptance criteria、checks、reviewer 状态与 evidence。
3. 把 build/test/lint 等命令结果结构化保存。

#### ralph 任务
1. 实现 iteration loop。
2. 将当前 story / acceptance tracking 写入 mode state。
3. 接入 verifier gate。
4. 支持失败重试、成功完成、恢复继续。

#### 交付物
- verification runtime
- ralph MVP

#### 验证
- ralph 可以在中途中断后继续迭代
- verifier record 能独立读取并判断完成状态
- acceptance criteria 与 evidence 可以对应起来

---

### Milestone 4：team MVP
**目标**：在共享底座之上实现最小 claw-native 团队编排。

#### 任务
1. 将 `TeamRegistry` 升级为可持久化 team runtime。
2. 为 team 增加 `phase`、`session_id` 与 task linkage。
3. 实现 task grouping、phase transition、aggregate status。
4. 打通 team 与 persistent task store / verification runtime 的连接。

#### 交付物
- team MVP
- team 运行态持久化
- team 状态视图

#### 验证
- team 创建后可恢复
- team 状态能正确反映 task 进度
- team 可作为后续 Phase 2 适配 tmux/external runtime 的基础

## 模块切分建议

### Workstream A：状态与持久化
- mode state
- task persistence
- team persistence
- runtime 索引

### Workstream B：澄清与执行
- deep-interview MVP
- ultrawork MVP

### Workstream C：验证与闭环
- verification runtime
- ralph MVP

### Workstream D：协调层
- team MVP

## 文件与目录约束
实施时遵循规格中的布局：

```text
.omx/state/
.omx/runtime/tasks/
.omx/runtime/teams/
.omx/runtime/verification/
.omx/specs/
.claw/sessions/
```

不要在 Phase 1 引入新的顶层运行时根目录。

## 明确延后到 Phase 2 的事项
以下内容只做接口预留，不在本计划中实现：
- `/oh-my-claudecode:*` slash command 兼容
- Claude Code plugin manifest 兼容
- `UserPromptSubmit` / `SessionStart` / `Stop` 等完整 OMC lifecycle hooks
- `omc-teams` / tmux / 外部 `claude` / `codex` / `gemini` worker runtime
- 完整 `notepad` / `project_memory` / `shared_memory` 生态

## 风险控制
1. **模式逻辑污染 registry**
   - 解决：把 registry 保持为数据访问层，把编排放入 runtime 层。
2. **过早做 OMC 外形兼容**
   - 解决：严格卡住 Phase 1 范围。
3. **没有 verification substrate 就实现 ralph**
   - 解决：必须在 Milestone 3 前半先落 verification runtime。
4. **team 复杂度提前爆炸**
   - 解决：只做 claw-native team MVP，不做外部 worker。

## 验收门槛
在开始编码前，执行任务必须满足：
- 规格文档已批准
- 本实现计划已批准
- 每个里程碑都有明确验证方法
- Phase 1 非目标仍然清晰，没有范围漂移

## 实施完成定义
当以下条件都满足时，Phase 1 视为完成：
1. 四个模式都有可运行 MVP。
2. 运行态、任务、团队、验证数据都可持久化。
3. 中断恢复路径已验证。
4. `deep-interview -> spec -> execution mode` 的交接链路已打通。
5. 为 Phase 2 的 OMC 兼容适配层保留了清晰接口，而不是把核心运行时绑死在 OMC 外形上。
