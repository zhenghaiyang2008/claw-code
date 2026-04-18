# `~/.claude/plugins` Follow-up 实施计划

> **给代理执行者：** 必须使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务逐项实现本计划。步骤使用复选框（`- [ ]`）跟踪。

**目标：** 在不改写现有 claw plugin registry / settings / installed.json / install_path 数据面的前提下，为 `~/.claude/plugins/*/.claude-plugin/plugin.json` 和 `CLAUDE_CONFIG_DIR/plugins/*/.claude-plugin/plugin.json` 增加只读发现与报告兼容。

**架构：** 保持当前 plugin manager 的“已安装插件目录 + registry/settings 数据面”不变，只把 Claude 用户目录作为额外只读发现来源注入现有扫描流程。manifest 规则继续严格要求 `*/.claude-plugin/plugin.json`，不扩展为“目录即插件根”。

**技术栈：** Rust workspace（`plugins`、必要时少量 `rusty-claude-cli` / `commands` 报告面）、现有 `load_plugin_from_directory()`、`plugin_manifest_path()`、`discover_plugin_dirs()`、`PluginManager::plugin_registry()`、`cargo test` / `cargo check`。

---

## 文件结构与职责划分

- `rust/crates/plugins/src/lib.rs`
  - 本轮主要修改点。
  - 扩展只读插件发现来源，把 Claude 用户级 plugins 目录纳入扫描。
  - 保持 registry/settings/install_root 语义不变。
- `docs/superpowers/specs/2026-04-18-claude-home-plugins-followup-design.md`
  - 已批准设计文档，仅引用，不改写。

本轮预计**不需要**改动 `runtime`、`tools` 或 plugin 安装命令的写入路径逻辑；只有在报告面缺失 Claude 来源描述时，才少量补 CLI 帮助/文本。

---

### 任务 1：把 Claude 用户级 plugin 目录纳入只读发现流程

**文件：**
- 修改：`rust/crates/plugins/src/lib.rs`
- 测试：`rust/crates/plugins/src/lib.rs`

- [ ] **步骤 1：先写一个会失败的 Claude home plugin 发现测试**

```rust
#[test]
fn list_installed_plugins_discovers_claude_home_packaged_plugins() {
    let config_home = temp_dir("claude-home-plugin-config");
    let home_root = temp_dir("claude-home-plugin-home");
    let claude_plugins_root = home_root.join(".claude").join("plugins");
    let plugin_root = claude_plugins_root.join("home-demo");

    write_file(
        plugin_root.join(".claude-plugin").join("plugin.json").as_path(),
        r#"{
  "name": "home-demo",
  "version": "1.0.0",
  "description": "Claude home plugin",
  "permissions": ["read"]
}"#,
    );

    let original_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home_root);

    let manager = PluginManager::new(PluginManagerConfig::new(config_home.clone()));
    let registry = manager.plugin_registry().expect("plugin registry should load");
    let ids = registry
        .definitions()
        .iter()
        .map(|definition| definition.metadata.id.as_str())
        .collect::<Vec<_>>();

    assert!(ids.iter().any(|id| *id == "home-demo@external"));

    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}
```

- [ ] **步骤 2：运行定向测试，确认它先失败**

运行：`cd rust && cargo test -p plugins list_installed_plugins_discovers_claude_home_packaged_plugins -- --nocapture`
预期：**FAIL**，因为当前插件发现只扫描 install root / registry 相关目录，不扫描 `~/.claude/plugins`。

- [ ] **步骤 3：在插件发现流程中加入 Claude 用户目录扫描辅助函数**

```rust
fn discover_claude_user_plugin_dirs() -> Result<Vec<PathBuf>, PluginError> {
    let mut roots = Vec::new();

    if let Ok(claude_config_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        roots.push(PathBuf::from(claude_config_dir).join("plugins"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".claude").join("plugins"));
    }

    let mut discovered = Vec::new();
    for root in roots {
        for path in discover_plugin_dirs(&root)? {
            if !discovered.iter().any(|existing| existing == &path) {
                discovered.push(path);
            }
        }
    }
    Ok(discovered)
}
```

- [ ] **步骤 4：把 Claude 用户目录接入现有 registry 组装路径，但保持只读语义**

```rust
for source_root in discover_claude_user_plugin_dirs()? {
    match load_plugin_definition(&source_root, PluginKind::External, source_root.display().to_string(), PluginKind::External.marketplace()) {
        Ok(definition) => discovered_definitions.push(definition),
        Err(error) => failures.push(PluginLoadFailure {
            install_path: source_root,
            error,
        }),
    }
}
```

要求：
- 只参与 list / registry 发现
- 不写 registry
- 不复制到 install root
- 不改变 `install_root()` / `registry_path()` / `settings_path()` 返回值

- [ ] **步骤 5：运行 plugins 定向测试确认发现逻辑通过**

运行：`cd rust && cargo test -p plugins list_installed_plugins_discovers_claude_home_packaged_plugins -- --nocapture`
预期：**PASS**。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/plugins/src/lib.rs
git commit -m "Discover Claude home plugins as read-only compatibility sources"
```

---

### 任务 2：把 Claude 用户级插件来源与状态正确暴露到报告面

**文件：**
- 修改：`rust/crates/plugins/src/lib.rs`
- 测试：`rust/crates/plugins/src/lib.rs`

- [ ] **步骤 1：写一个会失败的来源展示测试**

```rust
#[test]
fn plugin_registry_report_marks_claude_home_plugin_as_discovered_only() {
    let config_home = temp_dir("claude-home-plugin-report-config");
    let home_root = temp_dir("claude-home-plugin-report-home");
    let plugin_root = home_root
        .join(".claude")
        .join("plugins")
        .join("home-demo");

    write_file(
        plugin_root.join(".claude-plugin").join("plugin.json").as_path(),
        r#"{
  "name": "home-demo",
  "version": "1.0.0",
  "description": "Claude home plugin",
  "permissions": ["read"]
}"#,
    );

    let original_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home_root);

    let manager = PluginManager::new(PluginManagerConfig::new(config_home));
    let registry = manager.plugin_registry().expect("plugin registry should load");
    let report = registry.report();

    assert!(report.contains("home-demo"));
    assert!(report.contains("external"));
    assert!(report.contains("discovered"));

    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}
```

- [ ] **步骤 2：运行测试，确认报告面当前还不够准确**

运行：`cd rust && cargo test -p plugins plugin_registry_report_marks_claude_home_plugin_as_discovered_only -- --nocapture`
预期：如果 report 还不能区分来源或 discovered-only 状态，则 **FAIL**。

- [ ] **步骤 3：为 Claude 用户目录发现补充来源标签与状态表达**

```rust
let source = format!("claude-home:{}", source_root.display());
```

并在 report / summary 中确保：
- 来源可区分为 Claude 用户级来源
- 未安装、只被发现的插件不会伪装成已安装插件
- broken plugin / discovered-only plugin / registry-managed plugin 的状态能分开看懂

- [ ] **步骤 4：增加 project-over-user precedence 测试**

```rust
#[test]
fn project_plugins_still_outrank_claude_home_plugins() {
    // project root plugin 与 ~/.claude plugin 同名时
    // 断言 project plugin 生效，Claude home plugin 进入 shadowed / lower-priority 路径
}
```

- [ ] **步骤 5：运行 plugins 报告面定向回归**

运行：`cd rust && cargo test -p plugins -- --nocapture`
预期：**PASS**，包括新增加的 Claude home discovery / report / precedence 测试。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/plugins/src/lib.rs
git commit -m "Report Claude home plugins without changing plugin ownership semantics"
```

---

### 任务 3：锁定现有写入面不回归

**文件：**
- 修改：`rust/crates/plugins/src/lib.rs`
- 测试：`rust/crates/plugins/src/lib.rs`

- [ ] **步骤 1：写一个会失败的回归测试，证明 registry/settings/install_root 没被改写**

```rust
#[test]
fn claude_home_plugin_discovery_does_not_change_registry_or_install_paths() {
    let config_home = temp_dir("claude-home-plugin-paths-config");
    let home_root = temp_dir("claude-home-plugin-paths-home");
    let plugin_root = home_root
        .join(".claude")
        .join("plugins")
        .join("home-demo");

    write_file(
        plugin_root.join(".claude-plugin").join("plugin.json").as_path(),
        r#"{
  "name": "home-demo",
  "version": "1.0.0",
  "description": "Claude home plugin",
  "permissions": ["read"]
}"#,
    );

    let original_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home_root);

    let config = PluginManagerConfig::new(config_home.clone());
    let manager = PluginManager::new(config.clone());
    let _ = manager.plugin_registry().expect("registry should load");

    assert_eq!(manager.install_root(), config_home.join("plugins").join("installed"));
    assert_eq!(manager.registry_path(), config_home.join("plugins").join("installed.json"));
    assert_eq!(manager.settings_path(), config_home.join("settings.json"));

    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}
```

- [ ] **步骤 2：运行定向测试，确认写入面保持稳定**

运行：`cd rust && cargo test -p plugins claude_home_plugin_discovery_does_not_change_registry_or_install_paths -- --nocapture`
预期：**PASS**。

- [ ] **步骤 3：在发现代码里避免把 Claude 用户目录写入 registry**

```rust
// 只把 Claude 用户级插件加入内存中的 discovered definitions / load failures
// 不调用 save_registry()
// 不把它们复制到 install_root()
```

- [ ] **步骤 4：运行最终定向验证集合**

运行：
```bash
cd rust
cargo test -p plugins list_installed_plugins_discovers_claude_home_packaged_plugins -- --nocapture
cargo test -p plugins plugin_registry_report_marks_claude_home_plugin_as_discovered_only -- --nocapture
cargo test -p plugins claude_home_plugin_discovery_does_not_change_registry_or_install_paths -- --nocapture
cargo check -p plugins -p rusty-claude-cli --quiet
git diff --check
```
预期：**PASS**。

- [ ] **步骤 5：提交本任务**

```bash
git add rust/crates/plugins/src/lib.rs
git commit -m "Lock Claude home plugin compatibility to read-only discovery"
```

---

## 自检清单

- [ ] 设计覆盖检查：
  - `~/.claude/plugins/*/.claude-plugin/plugin.json` 的 discovery 已覆盖
  - 报告面来源与状态展示已覆盖
  - project-over-user precedence 已覆盖
  - registry / settings / install_root 不回归已覆盖
- [ ] 占位词扫描：无 TBD / TODO / “以后补”
- [ ] 类型与边界一致性：
  - 继续复用 `plugin_manifest_path()` / `load_plugin_from_directory()`
  - Claude home plugin 仅作为 discovered source，不写 registry
  - 写入职责仍保留在现有 claw plugin manager 数据面

## 执行交接

计划已保存到 `docs/superpowers/plans/2026-04-18-claude-home-plugins-followup-implementation-plan.md`。两种执行方式：

1. **Subagent-Driven（推荐）** —— 每个任务派发一个全新 subagent，中间带评审，迭代快
2. **Inline Execution** —— 在当前会话里用 executing-plans 按批次执行

你选哪一种？
