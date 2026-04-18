# `/` + `Tab` Top-Level Slash Commands 实施计划

> **给代理执行者：** 必须使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务逐项实现本计划。步骤使用复选框（`- [ ]`）跟踪。

**目标：** 在 REPL 中，当用户输入 `/` 并按一次 `Tab` 时，用现有 `rustyline` completion 列表样式展示全部顶层 slash commands，而不破坏现有前缀补全逻辑。

**架构：** 保持候选来源生成逻辑不变，只在 `SlashCommandHelper::complete()` 中对 `prefix == "/"` 加一个最小特殊分支。顶层命令通过“以 `/` 开头且不包含空格”来判定，避免引入新的命令注册或 UI 面板。

**技术栈：** Rust workspace、`rustyline`、`rust/crates/rusty-claude-cli/src/input.rs`、现有 `SlashCommandHelper` 单元测试。

---

## 文件结构与职责划分

- `rust/crates/rusty-claude-cli/src/input.rs`
  - 本轮唯一需要改动的行为文件。
  - 负责 slash completion 的过滤逻辑。
  - 新增 `/` 特殊补全分支与对应单元测试。
- `docs/superpowers/specs/2026-04-18-slash-tab-top-level-commands-design.md`
  - 已批准设计文档，仅引用，不改写。

本轮预计**不需要**改动 `main.rs`、slash command registry、direct CLI、或 REPL 主循环。

---

### 任务 1：为 `/` 顶层补全行为写失败测试并实现最小过滤分支

**文件：**
- 修改：`rust/crates/rusty-claude-cli/src/input.rs`
- 测试：`rust/crates/rusty-claude-cli/src/input.rs`

- [ ] **步骤 1：先写一个会失败的 `/` 顶层补全测试**

```rust
#[test]
fn slash_only_completion_lists_all_top_level_commands() {
    let helper = SlashCommandHelper::new(vec![
        "/help".to_string(),
        "/status".to_string(),
        "/mcp".to_string(),
        "/mcp list".to_string(),
        "/model".to_string(),
        "/model opus".to_string(),
        "/session".to_string(),
        "/session switch alpha".to_string(),
    ]);
    let history = DefaultHistory::new();
    let ctx = Context::new(&history);

    let (start, matches) = helper.complete("/", 1, &ctx).expect("completion should work");

    assert_eq!(start, 0);
    assert_eq!(
        matches
            .into_iter()
            .map(|candidate| candidate.replacement)
            .collect::<Vec<_>>(),
        vec![
            "/help".to_string(),
            "/status".to_string(),
            "/mcp".to_string(),
            "/model".to_string(),
            "/session".to_string(),
        ]
    );
}
```

- [ ] **步骤 2：运行定向测试，确认它先失败**

运行：`cd rust && cargo test -p rusty-claude-cli slash_only_completion_lists_all_top_level_commands -- --nocapture`
预期：**FAIL**，因为当前实现只按 `starts_with("/")` 返回所有 slash 候选，子命令模板也会混进来。

- [ ] **步骤 3：在 `SlashCommandHelper::complete()` 中加入 `/` 特殊分支**

```rust
let matches = if prefix == "/" {
    self.completions
        .iter()
        .filter(|candidate| !candidate.contains(' '))
        .map(|candidate| Pair {
            display: candidate.clone(),
            replacement: candidate.clone(),
        })
        .collect()
} else {
    self.completions
        .iter()
        .filter(|candidate| candidate.starts_with(prefix))
        .map(|candidate| Pair {
            display: candidate.clone(),
            replacement: candidate.clone(),
        })
        .collect()
};
```

- [ ] **步骤 4：补一个“不破坏普通前缀补全”的保护测试**

```rust
#[test]
fn slash_only_special_case_does_not_change_prefixed_matching() {
    let helper = SlashCommandHelper::new(vec![
        "/mcp".to_string(),
        "/mcp list".to_string(),
        "/model".to_string(),
        "/model opus".to_string(),
    ]);
    let history = DefaultHistory::new();
    let ctx = Context::new(&history);

    let (_, matches) = helper.complete("/m", 2, &ctx).expect("completion should work");
    let values = matches
        .into_iter()
        .map(|candidate| candidate.replacement)
        .collect::<Vec<_>>();

    assert!(values.contains(&"/mcp".to_string()));
    assert!(values.contains(&"/mcp list".to_string()));
    assert!(values.contains(&"/model".to_string()));
    assert!(values.contains(&"/model opus".to_string()));
}
```

- [ ] **步骤 5：运行 input.rs 定向验证**

运行：`cd rust && cargo test -p rusty-claude-cli complete -- --nocapture`
预期：**PASS**，包括现有 `/m`、`/model o`、非 slash 补全测试都继续通过。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/rusty-claude-cli/src/input.rs
git commit -m "List top-level slash commands when slash-only completion is requested"
```

---

### 任务 2：增加一次最小集成验证，证明候选源与显示行为仍匹配现有 REPL 补全集

**文件：**
- 修改：`rust/crates/rusty-claude-cli/src/input.rs`
- 测试：`rust/crates/rusty-claude-cli/src/main.rs`

- [ ] **步骤 1：增加一个最小集成测试，验证 `/` 只展示单段候选**

```rust
#[test]
fn slash_only_completion_uses_top_level_candidates_from_repl_source() {
    let completions = slash_command_completion_candidates_with_sessions(
        "sonnet",
        Some("session-current"),
        vec!["session-old".to_string()],
    );
    let helper = SlashCommandHelper::new(completions);
    let history = DefaultHistory::new();
    let ctx = Context::new(&history);

    let (_, matches) = helper.complete("/", 1, &ctx).expect("completion should work");
    let values = matches
        .into_iter()
        .map(|candidate| candidate.replacement)
        .collect::<Vec<_>>();

    assert!(values.contains(&"/help".to_string()));
    assert!(values.contains(&"/mcp".to_string()));
    assert!(values.contains(&"/skills".to_string()));
    assert!(!values.iter().any(|value| value.contains(' ')));
}
```

- [ ] **步骤 2：运行最小集成测试，确认它覆盖实际候选源**

运行：`cd rust && cargo test -p rusty-claude-cli slash_only_completion_uses_top_level_candidates_from_repl_source -- --nocapture`
预期：**PASS**。

- [ ] **步骤 3：运行 `rusty-claude-cli` 的针对性回归命令**

运行：
```bash
cd rust
cargo test -p rusty-claude-cli slash_only_completion_lists_all_top_level_commands -- --nocapture
cargo test -p rusty-claude-cli slash_only_special_case_does_not_change_prefixed_matching -- --nocapture
cargo test -p rusty-claude-cli completion_candidates_include_workflow_shortcuts_and_dynamic_sessions -- --nocapture
cargo check -p rusty-claude-cli --quiet
git diff --check
```
预期：**PASS**。

- [ ] **步骤 4：提交本任务**

```bash
git add rust/crates/rusty-claude-cli/src/input.rs rust/crates/rusty-claude-cli/src/main.rs
git commit -m "Verify slash-only completion against the REPL candidate source"
```

---

## 自检清单

- [ ] 设计覆盖检查：
  - `/` + `Tab` 只列顶层命令已覆盖
  - `/m` / `/model o` 等现有前缀补全不回归已覆盖
  - direct CLI 路径未被纳入改动范围
- [ ] 占位词扫描：无 TBD / TODO / “以后补”
- [ ] 边界一致性：
  - 只动 `input.rs` 的补全过滤逻辑
  - 候选来源仍由现有 REPL completion generator 提供
  - 顶层命令定义始终是“slash 开头且不含空格”

## 执行交接

计划已保存到 `docs/superpowers/plans/2026-04-18-slash-tab-top-level-commands-implementation-plan.md`。两种执行方式：

1. **Subagent-Driven（推荐）** —— 每个任务派发一个全新 subagent，中间带评审，迭代快
2. **Inline Execution** —— 在当前会话里用 executing-plans 按批次执行

你选哪一种？
