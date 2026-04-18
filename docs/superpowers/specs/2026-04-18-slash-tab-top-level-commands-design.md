# `/` + `Tab` Top-Level Slash Commands Design

## 背景

当前 `claw` 的 REPL 补全基于 `rustyline`，补全逻辑集中在：

- `rust/crates/rusty-claude-cli/src/input.rs`
- `SlashCommandHelper::complete()`
- `slash_command_prefix()`

现有行为是：
- 只要当前输入以 `/` 开头，且光标在行尾，就进入 slash completion
- 补全候选通过 `candidate.starts_with(prefix)` 过滤

这意味着输入 `/m` 后可以补全 `/mcp`、`/model` 等，但只输入 `/` 时，体验还不像 Claude 那样“按一次 Tab 立即看到全部顶层命令候选”。

## 目标

在 **REPL 模式** 下实现以下交互：

- 用户输入 `/`
- 按一次 `Tab`
- 直接列出**全部顶层 slash commands**

并满足：
- 只作用于 REPL
- 继续复用现有 `rustyline` completion 列表样式
- 不影响现有前缀补全逻辑

## 非目标

本次不做：

- 输入 `/` 自动弹出候选
- 展开子命令模板（如 `/mcp list`、`/session switch ...`）
- 改造 direct CLI slash 行为（如 `claw /skills`）
- 新的 Claude 风格命令面板 UI
- slash command registry 重构

## 交互定义

### 目标行为

#### 情况 A：输入恰好为 `/`
- 按 `Tab`
- 显示全部**顶层** slash commands

#### 情况 B：输入为 `/m`、`/pl` 等
- 仍保持现有前缀匹配
- 例如 `/m` 只展示 `/m...` 开头候选

#### 情况 C：输入普通文本
- 不触发 slash completion

### 顶层命令的定义

“顶层 slash commands” 定义为：
- 候选字符串以 `/` 开头
- 候选字符串中**不包含空格**

例如保留：
- `/help`
- `/status`
- `/sandbox`
- `/mcp`
- `/skills`
- `/plugin`
- `/agents`
- `/session`

例如排除：
- `/mcp list`
- `/model opus`
- `/session switch alpha`

## 设计方案

采用**最小侵入式改动**：

### 改动点

主要只改：
- `rust/crates/rusty-claude-cli/src/input.rs`

核心策略：
- 保持 `slash_command_prefix()` 不变
- 保持候选来源生成逻辑不变
- 只在 `SlashCommandHelper::complete()` 中增加一个特殊分支：
  - 当 `prefix == "/"` 时，返回“所有不含空格的 slash 候选”
  - 其它情况继续走原有 `starts_with(prefix)` 逻辑

### 为什么选这个方案

1. **改动最小**
   - 不动 slash command registry
   - 不动 direct CLI
   - 不动 REPL 主循环

2. **行为清晰**
   - `/` + `Tab` 是特殊入口
   - `/m` + `Tab` 保持旧逻辑

3. **测试简单**
   - 只需给 `complete()` 增加定向单测
   - 不需要复杂集成改造

## 模块边界

### `input.rs`
职责：
- 处理 slash completion 的展示过滤规则
- 新增 `/` 的特殊补全逻辑

### `main.rs`
职责保持不变：
- 继续提供 completion candidates
- 不需要新增候选来源或交互状态

## 测试与验收

### 单元测试

在 `rust/crates/rusty-claude-cli/src/input.rs` 中新增测试：

1. **`/` 返回全部顶层命令**
   - 输入 `/`
   - 结果应包含 `/help`、`/status`、`/mcp`
   - 不应包含 `/mcp list`、`/model opus`

2. **普通前缀补全不变**
   - `/m` 仍然只匹配 `/m...`
   - `/model o` 仍然只匹配 `/model opus`

3. **非 slash 输入不变**
   - `hello` 不触发 slash completion

4. **去重和规范化不变**
   - 复用现有 `normalize_completions()` 结果
   - 不把非 slash 候选带进来

### 人工验收

在真实 REPL 中：

```bash
cd rust
./target/debug/claw
```

输入：
```text
/
```
然后按一次 `Tab`

预期：
- 看到完整的顶层 slash commands 候选列表
- 不看到子命令模板
- 仍能继续正常输入和执行命令

## 成功标准

> 在 REPL 中，输入 `/` 后按一次 `Tab`，能用现有 completion 列表样式展示全部顶层 slash commands，且不破坏现有前缀补全行为。

## 风险与缓解

### 风险 1：把子命令模板也一起带出来
**缓解：** 明确使用“是否包含空格”作为顶层过滤条件。

### 风险 2：破坏现有 `/m`、`/model o` 等前缀补全
**缓解：** 仅对 `prefix == "/"` 走特殊分支，其余逻辑不变。

### 风险 3：候选列表里混入非 slash 项
**缓解：** 继续依赖现有 `normalize_completions()` 只保留 `/` 开头项。
