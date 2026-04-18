# `~/.claude` Home Compatibility Design

## 背景

当前 `claw-code` 已经部分支持 `~/.claude` 生态：

- `~/.claude/skills`、`~/.claude/commands`、`~/.claude/agents` 已有发现基础
- `.claude-plugin/plugin.json` 已有 manifest 兼容入口
- `mcpServers` 已有成熟的配置解析与 merge 机制

但这些能力还没有被整理成一套完整、稳定、可回归验证的 `~/.claude` 目录兼容设计。用户仍然需要额外搬运配置，或面对“能发现但不一定真可用”的不一致体验。

本设计的目标是在**不推翻 claw 现有运行时模型**的前提下，补齐 `~/.claude` 的行为兼容与管理面兼容。

---

## 目标

本轮目标覆盖：

- `~/.claude/skills`
- `~/.claude/agents`
- `~/.claude/commands`
- `~/.claude/settings.json` / `CLAUDE_CONFIG_DIR/settings.json` 中的 `mcpServers`

并满足两类兼容：

1. **运行时兼容**
   - 能发现、加载、解析、调用
2. **管理面兼容**
   - `/skills`、`/agents`、`/mcp` 等视图中能正确展示来源、覆盖关系与有效项

---

## 非目标

本轮不做：

- 100% 复刻 Claude / OMC 的所有隐式行为
- 引入第二套独立于 `ConfigLoader` 的 MCP 机制
- 把 `~/.claude` 升格为新的默认写入主入口
- 将 `~/.claude/commands` 提升为新的原生 slash command registry
- 完整实现 `~/.claude/plugins` 兼容（仅记录计划，不进入本轮实现）
- 热重载、目录监听、自动刷新

---

## 已评估方案

### 方案 A：最小侵入兼容层（选中）

在现有 `commands` / `runtime` / `tools` / `plugins` 结构上，增量纳入 `~/.claude` 来源，不重建运行时。

**优点**
- 改动面最小
- 回归风险最低
- 与当前架构最一致

**缺点**
- 是 claw 风格的兼容实现，不是上游原样复刻

### 方案 B：统一定义注册中心

为 `.claw` / `.codex` / `.omc` / `.agents` / `.claude` 建立统一 registry，再让 skills / agents / commands / plugins / MCP 全部走抽象层。

**优点**
- 长期结构最整齐
- 后续扩展最好

**缺点**
- 本轮改动过大，容易超 scope

### 方案 C：`.claude` 专用兼容模式

引入专门的 Claude-compatible mode，让 `~/.claude` 进入一套单独优先级或单独运行时模式。

**优点**
- 用户感知最强

**缺点**
- 会让当前 claw 行为模型分裂成两套

---

## 选定设计基线

### 总体兼容策略

采用：**以 claw 现有模型为主，吸收必要的 `~/.claude` 兼容语义**。

这意味着：

- `~/.claude` 是完整兼容来源，但不是最高优先级来源
- 兼容目标是“行为兼容 + 管理面兼容”
- 不承诺复刻所有上游未文档化细节

### 优先级策略

采用：

- **项目级 > 用户级**
- 同层顺序沿用当前 claw 既有顺序
- `~/.claude` 不压过项目级 `.claw` / `.codex` / `.omc` / `.agents`

### 写入策略

采用：**只读兼容为主**。

- `~/.claude/skills` / `agents` / `commands`：本轮只负责发现、展示、加载、调用
- 不将安装/管理动作默认写回 `~/.claude`
- `mcpServers`：从 `~/.claude/settings.json` 读取并 merge，不要求 claw 管理命令回写

### `commands` 语义

采用：

- `~/.claude/commands` 继续作为 **legacy command / skill 来源**
- 不提升为新的原生 slash command 注册体系

### `agents` 验收语义

采用更严格的标准：

- 不只要求 `/agents list` 可见
- 还要求进入现有 agent/subagent 使用链路时**真实可用**

### 插件策略

本轮不实现 `plugins`，但在 spec 中保留完整计划：

- 后续支持 `~/.claude/plugins/*/.claude-plugin/plugin.json`
- 继续保留 claw 自己的 plugin registry / settings / installed.json 数据面
- 不把 `~/.claude/plugins` 变成新的插件主写入面

---

## 模块设计

### 1. Definition Discovery Adapter

**主要落点**：`rust/crates/commands/src/lib.rs`

职责：

- 把 `~/.claude/skills`
- `~/.claude/agents`
- `~/.claude/commands`

纳入现有 discovery 流程，并保持当前 shadowing 语义。

重点复用：

- `discover_skill_roots`
- `load_skills_from_roots`
- `load_agents_from_roots`
- `resolve_skill_path`
- `handle_agents_slash_command`

### 2. Usability Bridge

**主要落点**：`commands` + `tools`，必要时少量补 `rusty-claude-cli`

职责：

- skill：discover 后必须能 resolve 和 invoke
- agent：discover 后必须能进入现有 agent/subagent 执行链路
- command：继续通过 legacy command / skill 调用链工作

### 3. Claude MCP Config Adapter

**主要落点**：`rust/crates/runtime/src/config.rs`

职责：

- 将 `~/.claude/settings.json`
- 以及 `CLAUDE_CONFIG_DIR/settings.json`

中的 `mcpServers` 纳入现有 `ConfigLoader` merge 链。

不新增第二套 MCP loader。

### 4. Deferred Plugin Compatibility Plan

**本轮不实现**，但后续设计在 spec 中固化：

- 只兼容 `~/.claude/plugins/*/.claude-plugin/plugin.json`
- 继续保留现有 plugin registry / settings / install state 数据面
- 后续单独规划实现周期

---

## 发现顺序与覆盖规则

### skills / commands

沿用当前 `discover_skill_roots()` 逻辑：

- 项目级 `.claw` / `.omc` / `.agents` / `.codex` / `.claude`
- 用户级 `CLAW_CONFIG_HOME` / `CODEX_HOME` / `~/.claw` / `~/.omc` / `~/.codex` / `~/.claude` / `CLAUDE_CONFIG_DIR`

其中：

- `~/.claude/skills` 为 `SkillsDir`
- `~/.claude/commands` 为 `LegacyCommandsDir`

同名项保持“先到先赢，后到 shadowed”。

### agents

沿用当前 `load_agents_from_roots()`：

- 先发现的同名 agent 生效
- 后发现的同名 agent 标记 `shadowed_by`
- `~/.claude/agents` 被纳入用户级来源，但不压过项目级来源

### MCP

`~/.claude/settings.json` 的 `mcpServers`：

- 作为用户级 Claude 配置来源参与 merge
- 不改变项目级 `.claw/settings.json` / `.claw/settings.local.json` 的优先级

### plugins（计划）

后续也必须遵循相同原则：

- 项目级 > 用户级
- `~/.claude` 只是兼容来源之一
- 不提升为插件主数据面

---

## 可用性定义

### skills

必须同时满足：

- `/skills list` 可见
- 来源与 shadowing 可解释
- `/skills <name> ...` 可真正解析并调用
- `resolve_skill_path()` 可落到 `~/.claude/skills/.../SKILL.md`

### commands

作为 legacy command / skill 来源时，必须满足：

- 能被 `/skills` 体系发现
- 能通过 skill / legacy-command 链路调用
- 不要求变成新的原生 slash command

### agents

必须同时满足：

- `/agents list` 可见
- 同名覆盖关系可见
- 进入现有 agent/subagent 使用链路时真实可用
- 不能只停留在 catalog 层

### MCP

必须满足：

- `/mcp list` 可见
- `/mcp show <server>` 可展示
- 进入 merge 后与现有 `.claw` 来源的 MCP server 等价可用
- 用户无需再重复配置一份

---

## MCP 合并规则

本轮对 `mcpServers` 的兼容语义为：

- 新增读取：
  - `~/.claude/settings.json`
  - `CLAUDE_CONFIG_DIR/settings.json`
- 只抽取 `mcpServers` 并纳入现有配置树
- 继续由 `ConfigLoader` 统一解析、校验、merge
- 不单独设计第二条 MCP 配置链

### 预期结果

如果用户已经把 MCP server 配在 `~/.claude/settings.json` 中：

- claw 启动后直接能看到这些 server
- `/mcp list` / `/mcp show` 可工作
- 运行时 MCP discovery / lifecycle 可直接使用

---

## Plugins 延后计划（记录，不实现）

本轮不实现 plugins，但必须在 spec 中记录后续边界：

### 后续兼容目标

- 支持发现：`~/.claude/plugins/*/.claude-plugin/plugin.json`

### 保持不变的数据面

- `settings.json`
- `installed.json`
- `install_path`
- 当前 plugin registry / enable-disable / install-state 模型

### 本轮不做

- 不把 `~/.claude/plugins` 作为默认插件写入根
- 不在本轮打通完整 plugin lifecycle 对齐
- 不扩展到更宽松的“目录即插件根”发现模式

---

## 验收标准

### 本轮必须通过的验收

#### skills
- `~/.claude/skills` 可发现、列出、解析、调用

#### agents
- `~/.claude/agents` 可发现、列出，并进入真实 agent/subagent 使用链路

#### commands
- `~/.claude/commands` 可作为 legacy command / skill 来源被发现并调用

#### MCP
- `~/.claude/settings.json` 中的 `mcpServers` 可进入现有 merge 链
- `/mcp list` / `/mcp show` 与运行时实际可用

#### 管理面
- `/skills`、`/agents`、`/mcp` 中来源与 shadowing 信息可解释

### 本轮明确不要求通过的验收

- plugins 实际加载与管理
- `~/.claude` 成为默认写入主入口
- Claude/OMC 所有隐式行为逐项对齐

---

## 风险与缓解

### 风险 1：发现顺序改动导致现有 shadowing 语义回归
**缓解**：坚持“只补来源，不改既有优先级模型”，并为 shadowing 写明确测试。

### 风险 2：MCP 配置源变多后 merge 结果变得不可预测
**缓解**：把 `~/.claude/settings.json` 视为现有用户级配置源之一，而不是新的最高优先级源；显式记录 precedence。

### 风险 3：agents 只在 list 层可见，但执行链路实际不可用
**缓解**：把“进入真实 agent/subagent 使用链路”列为独立验收项与测试项。

### 风险 4：plugins scope 失控
**缓解**：本轮只在 spec 中保留后续计划，不进入实现计划。

---

## 测试策略

### 1. discovery / reporting tests
- `~/.claude/skills` 出现在 `/skills list`
- `~/.claude/commands` 以 legacy command 来源出现在 `/skills`
- `~/.claude/agents` 出现在 `/agents list`
- shadowed 项在报告中正确标注

### 2. resolution / invocation tests
- `resolve_skill_path()` 可解析 `~/.claude/skills`
- legacy commands 可通过现有 invocation 路径调用
- `~/.claude/agents` 可进入 agent/subagent 使用链路

### 3. MCP config tests
- `~/.claude/settings.json` 中的 `mcpServers` 被合并进配置
- `/mcp list` / `/mcp show` 可见并输出正确来源结果
- 项目级 `.claw/settings.local.json` 能继续覆盖用户级 Claude 配置

### 4. regression tests
- 现有 `.claw` / `.codex` / `.omc` / `.agents` 来源不回归
- 既有 install root / registry 路径不被意外改写

---

## 实施范围建议

本轮 implementation plan 只覆盖：

1. `~/.claude/skills`
2. `~/.claude/agents`
3. `~/.claude/commands`
4. `~/.claude/settings.json` MCP merge
5. 对应 discovery / reporting / invocation / config 测试

plugins 仅作为**下一轮子项目计划**保留在 spec 中。
