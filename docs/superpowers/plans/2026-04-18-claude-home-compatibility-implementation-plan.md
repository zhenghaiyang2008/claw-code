# `~/.claude` Home Compatibility 实施计划

> **给代理执行者：** 必须使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务逐项实现本计划。步骤使用复选框（`- [ ]`）跟踪。

**目标：** 让 `claw` 通过现有 claw-native 的发现与配置系统，加载并使用 `~/.claude` 中的 skills、agents、commands，以及 MCP server 配置。

**架构：** 扩展当前 `commands` 发现层与 `runtime::ConfigLoader`，而不是创建第二套 Claude 专用运行时。保持“项目级优先于用户级”的既有优先级；保持 `~/.claude/commands` 作为 legacy command / skill 来源；本轮中所有 `~/.claude` 来源都保持只读兼容。插件兼容明确延期，但在已批准的设计中保留后续计划。

**技术栈：** Rust workspace（`commands`、`runtime`、`tools`、`rusty-claude-cli`）、现有 `ConfigLoader`、现有 skill/agent 发现函数、现有 `execute_agent_with_spawn` 执行路径、`cargo test` / `cargo check`。

---

## 文件结构与职责划分

- `rust/crates/runtime/src/config.rs`
  - 扩展配置发现逻辑，使 `~/.claude/settings.json` 与 `CLAUDE_CONFIG_DIR/settings.json` 能通过现有配置合并链参与 `mcpServers` 加载。
- `rust/crates/commands/src/lib.rs`
  - 规范并补强 `~/.claude` 在 skills、legacy commands、agents 上的发现与展示行为。
  - 更新帮助文本与报告输出，让管理面准确说明新的 MCP 来源链。
- `rust/crates/tools/src/lib.rs`
  - 确保运行时 skill lookup 的兼容根路径与 agent 执行测试覆盖 `~/.claude` 来源定义。
- `docs/superpowers/specs/2026-04-18-claude-home-compatibility-design.md`
  - 已批准的设计文档；实现时引用，不要改写。

本计划**刻意不修改** `rust/crates/plugins/src/lib.rs`。plugins 保持为后续子项目。

---

### 任务 1：把 Claude 用户配置文件纳入 MCP 合并链

**文件：**
- 修改：`rust/crates/runtime/src/config.rs`
- 测试：`rust/crates/runtime/src/config.rs`

- [ ] **步骤 1：先写一个会失败的 Claude MCP 来源发现测试**

```rust
#[test]
fn loads_and_merges_claude_home_mcp_servers_by_precedence() {
    let root = temp_dir();
    let cwd = root.join("project");
    let home = root.join("home").join(".claw");
    let claude_home = root.join("home").join(".claude");
    fs::create_dir_all(cwd.join(".claw")).expect("project config dir");
    fs::create_dir_all(&home).expect("home config dir");
    fs::create_dir_all(&claude_home).expect("claude home config dir");

    fs::write(
        claude_home.join("settings.json"),
        r#"{"mcpServers":{"claude-home":{"command":"uvx","args":["claude-home"]}}}"#,
    )
    .expect("write claude home settings");
    fs::write(
        cwd.join(".claw").join("settings.local.json"),
        r#"{"mcpServers":{"project-local":{"command":"uvx","args":["project-local"]}}}"#,
    )
    .expect("write project local settings");

    std::env::set_var("HOME", root.join("home"));
    let loaded = ConfigLoader::new(&cwd, &home).load().expect("config should load");

    assert!(loaded.mcp().get("claude-home").is_some());
    assert!(loaded.mcp().get("project-local").is_some());
}
```

- [ ] **步骤 2：运行定向测试，确认它先失败**

运行：`cd rust && cargo test -p runtime loads_and_merges_claude_home_mcp_servers_by_precedence -- --nocapture`
预期：**FAIL**，因为 `ConfigLoader::discover()` 目前还没有把 `~/.claude/settings.json` 纳入发现链。

- [ ] **步骤 3：扩展 `ConfigLoader::discover()`，加入 Claude 用户配置项**

```rust
pub fn discover(&self) -> Vec<ConfigEntry> {
    let user_legacy_path = self.config_home.parent().map_or_else(
        || PathBuf::from(".claw.json"),
        |parent| parent.join(".claw.json"),
    );
    let claude_home_settings = self
        .config_home
        .parent()
        .map(|parent| parent.join(".claude").join("settings.json"));
    let claude_env_settings = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .map(|dir| PathBuf::from(dir).join("settings.json"));

    let mut entries = vec![
        ConfigEntry {
            source: ConfigSource::User,
            path: user_legacy_path,
        },
        ConfigEntry {
            source: ConfigSource::User,
            path: self.config_home.join("settings.json"),
        },
    ];

    if let Some(path) = claude_home_settings {
        entries.push(ConfigEntry {
            source: ConfigSource::User,
            path,
        });
    }
    if let Some(path) = claude_env_settings {
        entries.push(ConfigEntry {
            source: ConfigSource::User,
            path,
        });
    }

    entries.extend([
        ConfigEntry {
            source: ConfigSource::Project,
            path: self.cwd.join(".claw.json"),
        },
        ConfigEntry {
            source: ConfigSource::Project,
            path: self.cwd.join(".claw").join("settings.json"),
        },
        ConfigEntry {
            source: ConfigSource::Local,
            path: self.cwd.join(".claw").join("settings.local.json"),
        },
    ]);
    entries
}
```

- [ ] **步骤 4：在现有 merge 测试中补充 Claude 配置优先级断言**

```rust
assert!(loaded.mcp().get("claude-home").is_some());
assert_eq!(loaded.loaded_entries()[1].source, ConfigSource::User);
assert!(loaded
    .loaded_entries()
    .iter()
    .any(|entry| entry.path.ends_with(Path::new(".claude/settings.json"))));
```

- [ ] **步骤 5：运行 runtime 配置相关定向验证**

运行：`cd rust && cargo test -p runtime config -- --nocapture`
预期：**PASS**，包括新的 Claude MCP 来源测试和现有优先级测试都通过。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/runtime/src/config.rs
git commit -m "Load Claude home MCP config through the existing merge chain"
```

---

### 任务 2：锁定 `~/.claude` skills 与 legacy commands 的可见性、覆盖显示与来源表达

**文件：**
- 修改：`rust/crates/commands/src/lib.rs`
- 测试：`rust/crates/commands/src/lib.rs`

- [ ] **步骤 1：为 Claude 用户级 skills + legacy commands 写一个会失败的报告测试**

```rust
#[test]
fn lists_claude_home_skills_and_legacy_commands_with_shadowing() {
    let workspace = temp_dir("claude-home-skills");
    let user_home = temp_dir("claude-home-user");
    let project_skills = workspace.join(".claw").join("skills");
    let user_skills = user_home.join(".claude").join("skills");
    let user_commands = user_home.join(".claude").join("commands");

    write_skill(&project_skills, "plan", "Project plan skill");
    write_skill(&user_skills, "plan", "User Claude plan skill");
    write_skill(&user_skills, "ralph", "User Claude ralph skill");
    write_markdown_command(&user_commands, "handoff", "Claude command handoff");

    std::env::set_var("HOME", &user_home);
    let roots = discover_skill_roots(&workspace);
    let report = render_skills_report(&load_skills_from_roots(&roots).expect("skills should load"));

    assert!(report.contains("ralph · User Claude ralph skill"));
    assert!(report.contains("handoff · Claude command handoff · legacy /commands"));
    assert!(report.contains("(shadowed by Project roots) plan · User Claude plan skill"));
}
```

- [ ] **步骤 2：运行定向 commands 测试，确认基线是否不足**

运行：`cd rust && cargo test -p commands lists_claude_home_skills_and_legacy_commands_with_shadowing -- --nocapture`
预期：如果报告文本、来源标签或 root 处理与已批准设计不完全一致，则先 **FAIL**。

- [ ] **步骤 3：规范 `/skills` 帮助文本，让 Claude 配置来源表达清楚**

```rust
fn render_skills_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        format_unexpected_line(unexpected),
        "Skills".to_string(),
        "  Usage            /skills [list|install <path>|help|<skill> [args]]".to_string(),
        "  Alias            /skill".to_string(),
        "  Direct CLI       claw skills [list|install <path>|help|<skill> [args]]".to_string(),
        "  Invoke           /skills help overview -> $help overview".to_string(),
        "  Install root     $CLAW_CONFIG_HOME/skills or ~/.claw/skills".to_string(),
        "  Sources          .claw/skills, .omc/skills, .agents/skills, .codex/skills, .claude/skills, ~/.claw/skills, ~/.omc/skills, ~/.codex/skills, ~/.claude/skills, ~/.claude/skills/omc-learned, CLAUDE_CONFIG_DIR/skills, legacy /commands".to_string(),
    ];
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}
```

- [ ] **步骤 4：补充 JSON/report 断言，锁定 Claude 用户来源标识**

```rust
assert_eq!(report["skills"][0]["source"]["scope"], "user_home");
assert_eq!(report["skills"][0]["source"]["id"], "user_claude");
```

- [ ] **步骤 5：运行 commands 的发现与帮助回归测试**

运行：`cd rust && cargo test -p commands -- --nocapture`
预期：**PASS**，现有 `/skills`、`/agents`、`/mcp` 的帮助与报告测试都通过。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/commands/src/lib.rs
git commit -m "Clarify Claude home discovery in skills and command reporting"
```

---

### 任务 3：让 `~/.claude` skills、commands、agents 能走通现有执行链路

**文件：**
- 修改：`rust/crates/tools/src/lib.rs`
- 修改：`rust/crates/commands/src/lib.rs`
- 测试：`rust/crates/tools/src/lib.rs`
- 测试：`rust/crates/commands/src/lib.rs`

- [ ] **步骤 1：先加一个会失败的 tools 侧 skill 解析测试，覆盖 `~/.claude/commands`**

```rust
#[test]
fn resolve_skill_path_from_compat_roots_finds_claude_home_legacy_commands() {
    let workspace = temp_path("claude-home-command-resolution");
    let user_home = temp_path("claude-home-command-user");
    let command_root = user_home.join(".claude").join("commands");
    write_markdown_command(&command_root, "handoff", "Claude handoff command");

    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&workspace).expect("set cwd");
    std::env::set_var("HOME", &user_home);

    let path = resolve_skill_path("handoff").expect("legacy command should resolve");
    assert!(path.ends_with(Path::new(".claude/commands/handoff.md")));

    std::env::set_current_dir(&original_dir).expect("restore cwd");
}
```

- [ ] **步骤 2：运行 tools 定向测试，确认它先失败**

运行：`cd rust && cargo test -p tools resolve_skill_path_from_compat_roots_finds_claude_home_legacy_commands -- --nocapture`
预期：如果 compat roots 或 command resolution 还不完整，则 **FAIL**。

- [ ] **步骤 3：补齐 tools 侧兼容 roots，使其与 commands 侧 Claude 路径完全对齐**

```rust
fn push_home_skill_lookup_roots(roots: &mut Vec<SkillLookupRoot>, home: &Path) {
    push_skill_lookup_root(roots, home.join(".claw").join("skills"), SkillLookupOrigin::SkillsDir);
    push_skill_lookup_root(roots, home.join(".omc").join("skills"), SkillLookupOrigin::SkillsDir);
    push_skill_lookup_root(roots, home.join(".codex").join("skills"), SkillLookupOrigin::SkillsDir);
    push_skill_lookup_root(roots, home.join(".claude").join("skills"), SkillLookupOrigin::SkillsDir);
    push_skill_lookup_root(roots, home.join(".claude").join("skills").join("omc-learned"), SkillLookupOrigin::SkillsDir);
    push_skill_lookup_root(roots, home.join(".claw").join("commands"), SkillLookupOrigin::LegacyCommandsDir);
    push_skill_lookup_root(roots, home.join(".codex").join("commands"), SkillLookupOrigin::LegacyCommandsDir);
    push_skill_lookup_root(roots, home.join(".claude").join("commands"), SkillLookupOrigin::LegacyCommandsDir);
}
```

- [ ] **步骤 4：添加一个 Claude agent 进入 spawn 路径的执行测试**

```rust
#[test]
fn agents_loaded_from_claude_home_enter_existing_spawn_path() {
    let workspace = temp_path("claude-home-agent-spawn");
    let user_home = temp_path("claude-home-agent-user");
    let agents_root = user_home.join(".claude").join("agents");
    write_agent_toml(&agents_root, "planner", "Claude planner", "gpt-5.4", "medium");

    std::env::set_var("HOME", &user_home);
    let report = commands::handle_agents_slash_command(Some("list"), &workspace).expect("agents list");
    assert!(report.contains("planner · Claude planner"));

    let output = execute_agent_with_spawn(
        AgentInput {
            description: "Use claude planner agent".to_string(),
            prompt: "Produce a planning summary".to_string(),
            subagent_type: Some("Plan".to_string()),
            name: Some("planner".to_string()),
            model: None,
        },
        |job| persist_agent_terminal_state(&job.manifest, "completed", Some("completed planning lane with context"), None),
    )
    .expect("agent should spawn");

    assert_eq!(output.status, "completed");
}
```

- [ ] **步骤 5：运行可用性验证**

运行：`cd rust && cargo test -p tools -- --nocapture`
预期：**PASS**，包括 Claude command resolution 与 agent spawn 路径覆盖都通过。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/tools/src/lib.rs rust/crates/commands/src/lib.rs
git commit -m "Keep Claude home definitions usable through existing execution paths"
```

---

### 任务 4：让 Claude MCP 来源在管理面里表达一致，并收敛最终回归验证

**文件：**
- 修改：`rust/crates/commands/src/lib.rs`
- 测试：`rust/crates/commands/src/lib.rs`
- 测试：`rust/crates/runtime/src/config.rs`

- [ ] **步骤 1：先写一个会失败的 `/mcp help` 来源字符串测试**

```rust
#[test]
fn mcp_usage_mentions_claude_settings_sources() {
    let help = render_mcp_usage(None);
    assert!(help.contains("~/.claude/settings.json"));
    assert!(help.contains("CLAUDE_CONFIG_DIR/settings.json"));
}
```

- [ ] **步骤 2：运行定向帮助测试，确认它先失败**

运行：`cd rust && cargo test -p commands mcp_usage_mentions_claude_settings_sources -- --nocapture`
预期：**FAIL**，因为当前帮助文本只列出了 `.claw/settings.json` 和 `.claw/settings.local.json`。

- [ ] **步骤 3：更新 MCP 帮助/报告文本，说明 Claude 用户级来源链**

```rust
fn render_mcp_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        format_unexpected_line(unexpected),
        "MCP".to_string(),
        "  Usage            /mcp [list|show <server>|help]".to_string(),
        "  Direct CLI       claw mcp [list|show <server>|help]".to_string(),
        "  Sources          ~/.claw/settings.json, ~/.claude/settings.json, CLAUDE_CONFIG_DIR/settings.json, .claw/settings.json, .claw/settings.local.json".to_string(),
    ];
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}
```

- [ ] **步骤 4：添加一个 merged-report 测试，证明 Claude MCP server 能通过 `/mcp show` 暴露**

```rust
#[test]
fn render_mcp_report_includes_claude_home_servers() {
    let root = temp_dir();
    let workspace = root.join("workspace");
    let claw_home = root.join("home").join(".claw");
    let claude_home = root.join("home").join(".claude");
    fs::create_dir_all(workspace.join(".claw")).expect("workspace config dir");
    fs::create_dir_all(&claw_home).expect("claw home");
    fs::create_dir_all(&claude_home).expect("claude home");

    fs::write(
        claude_home.join("settings.json"),
        r#"{"mcpServers":{"claude-home":{"command":"uvx","args":["claude-home"]}}}"#,
    )
    .expect("write claude settings");

    std::env::set_var("HOME", root.join("home"));
    let loader = ConfigLoader::new(&workspace, &claw_home).load().expect("config load");
    let show = render_mcp_report_for(&loader, &workspace, Some("show claude-home")).expect("mcp show");
    assert!(show.contains("Name              claude-home"));
    assert!(show.contains("Command           uvx"));
}
```

- [ ] **步骤 5：运行本功能集的最终定向回归**

运行：`cd rust && cargo test -p runtime config -- --nocapture && cargo test -p commands -- --nocapture && cargo test -p tools -- --nocapture && cargo check -p runtime -p commands -p tools -p rusty-claude-cli --quiet && git diff --check`
预期：**PASS**，Claude home 的发现、可用性、MCP 报告都被覆盖，且无 check/格式回归。

- [ ] **步骤 6：提交本任务**

```bash
git add rust/crates/runtime/src/config.rs rust/crates/commands/src/lib.rs rust/crates/tools/src/lib.rs
git commit -m "Finish Claude home compatibility for discovery, usability, and MCP reporting"
```

---

## 自检清单

- [ ] 已确认 spec 覆盖：
  - `~/.claude/skills` 的发现 + 可用性由任务 2、3 覆盖
  - `~/.claude/agents` 的发现 + 真正执行链路由任务 2、3 覆盖
  - `~/.claude/commands` 的 legacy-command 兼容由任务 2、3 覆盖
  - `~/.claude/settings.json` / `CLAUDE_CONFIG_DIR/settings.json` 的 MCP 合并由任务 1、4 覆盖
  - plugins 被明确延期，未误入本轮任务
- [ ] 已完成占位词扫描：没有 TBD / TODO / “以后补”
- [ ] 类型与边界一致性已检查：
  - `ConfigLoader::discover()` 仍是唯一配置来源发现入口
  - `resolve_skill_path()` 与 compat roots 使用的 Claude 路径与 `discover_skill_roots()` 对齐
  - agent 执行继续复用 `execute_agent_with_spawn()`，不引入新运行时路径

## 执行交接

计划已保存到 `docs/superpowers/plans/2026-04-18-claude-home-compatibility-implementation-plan.md`。两种执行方式：

1. **Subagent-Driven（推荐）** —— 每个任务派发一个全新 subagent，中间带评审，迭代快
2. **Inline Execution** —— 在当前会话里用 executing-plans 按批次执行

你选哪一种？
