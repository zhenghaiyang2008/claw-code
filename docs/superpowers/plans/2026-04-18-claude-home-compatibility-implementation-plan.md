# `~/.claude` Home Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `claw` load and use `~/.claude` skills, agents, commands, and MCP server config through the existing claw-native discovery and config systems.

**Architecture:** Extend the current `commands` discovery layer and `runtime::ConfigLoader` rather than creating a second Claude-specific runtime. Keep project-over-user precedence, keep `~/.claude/commands` as legacy command / skill sources, and keep all `~/.claude` sources read-only in this cycle. Plugin compatibility is explicitly deferred but recorded in the approved design.

**Tech Stack:** Rust workspace (`commands`, `runtime`, `tools`, `rusty-claude-cli`), existing `ConfigLoader`, existing skill/agent discovery functions, existing `execute_agent_with_spawn` runtime path, cargo test/cargo check.

---

## File structure and responsibility map

- `rust/crates/runtime/src/config.rs`
  - Extend config discovery so `~/.claude/settings.json` and `CLAUDE_CONFIG_DIR/settings.json` participate in the existing config merge chain for `mcpServers`.
- `rust/crates/commands/src/lib.rs`
  - Normalize and document `~/.claude` discovery/reporting behavior for skills, legacy commands, and agents.
  - Update help/reporting strings so management surfaces describe the new MCP source chain accurately.
- `rust/crates/tools/src/lib.rs`
  - Ensure the runtime-facing skill lookup compatibility roots and agent execution tests cover `~/.claude` sourced definitions.
- `docs/superpowers/specs/2026-04-18-claude-home-compatibility-design.md`
  - Approved design artifact; reference only, do not rewrite during implementation.

This plan intentionally avoids touching `rust/crates/plugins/src/lib.rs` in this cycle. Plugins remain a deferred follow-up.

---

### Task 1: Add Claude user config files to the MCP merge chain

**Files:**
- Modify: `rust/crates/runtime/src/config.rs`
- Test: `rust/crates/runtime/src/config.rs`

- [ ] **Step 1: Write the failing config discovery test for Claude MCP sources**

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

- [ ] **Step 2: Run the targeted config test to verify it fails first**

Run: `cd rust && cargo test -p runtime loads_and_merges_claude_home_mcp_servers_by_precedence -- --nocapture`
Expected: FAIL because `ConfigLoader::discover()` does not yet include `~/.claude/settings.json`.

- [ ] **Step 3: Extend `ConfigLoader::discover()` with Claude user config entries**

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

- [ ] **Step 4: Add precedence assertions for Claude config in the existing merge test**

```rust
assert!(loaded.mcp().get("claude-home").is_some());
assert_eq!(loaded.loaded_entries()[1].source, ConfigSource::User);
assert!(loaded
    .loaded_entries()
    .iter()
    .any(|entry| entry.path.ends_with(Path::new(".claude/settings.json"))));
```

- [ ] **Step 5: Run the targeted runtime config verification**

Run: `cd rust && cargo test -p runtime config -- --nocapture`
Expected: PASS, including the new Claude MCP discovery test and existing precedence tests.

- [ ] **Step 6: Commit the config merge change**

```bash
git add rust/crates/runtime/src/config.rs
git commit -m "Load Claude home MCP config through the existing merge chain"
```

---

### Task 2: Lock in `~/.claude` skills and legacy commands as visible, shadow-aware discovery sources

**Files:**
- Modify: `rust/crates/commands/src/lib.rs`
- Test: `rust/crates/commands/src/lib.rs`

- [ ] **Step 1: Add a failing report test for user Claude skills + legacy commands**

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

- [ ] **Step 2: Run the targeted commands test to verify baseline behavior**

Run: `cd rust && cargo test -p commands lists_claude_home_skills_and_legacy_commands_with_shadowing -- --nocapture`
Expected: FAIL if report text or root handling does not yet fully match the approved spec wording.

- [ ] **Step 3: Normalize the user-facing `/skills` help text to mention Claude config sources explicitly**

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

- [ ] **Step 4: Add JSON/report assertions for the Claude user source id**

```rust
assert_eq!(report["skills"][0]["source"]["scope"], "user_home");
assert_eq!(report["skills"][0]["source"]["id"], "user_claude");
```

- [ ] **Step 5: Run the commands discovery and help regression suite**

Run: `cd rust && cargo test -p commands -- --nocapture`
Expected: PASS, including existing `/skills`, `/agents`, and `/mcp` help/report tests.

- [ ] **Step 6: Commit the discovery/reporting update**

```bash
git add rust/crates/commands/src/lib.rs
git commit -m "Clarify Claude home discovery in skills and command reporting"
```

---

### Task 3: Make `~/.claude` skills, commands, and agents usable through the existing execution paths

**Files:**
- Modify: `rust/crates/tools/src/lib.rs`
- Modify: `rust/crates/commands/src/lib.rs`
- Test: `rust/crates/tools/src/lib.rs`
- Test: `rust/crates/commands/src/lib.rs`

- [ ] **Step 1: Add a failing tool-side skill resolution test for `~/.claude/commands`**

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

- [ ] **Step 2: Run the targeted tools test to verify it fails first**

Run: `cd rust && cargo test -p tools resolve_skill_path_from_compat_roots_finds_claude_home_legacy_commands -- --nocapture`
Expected: FAIL if the compat root order or command resolution does not yet cover the test case.

- [ ] **Step 3: Extend tool-side compat roots to mirror commands-side Claude paths exactly**

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

- [ ] **Step 4: Add an execution-path test for Claude-sourced agents entering the spawn path**

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

- [ ] **Step 5: Run the targeted usability verification**

Run: `cd rust && cargo test -p tools -- --nocapture`
Expected: PASS, including Claude command resolution and agent spawn-path coverage.

- [ ] **Step 6: Commit the execution-path compatibility change**

```bash
git add rust/crates/tools/src/lib.rs rust/crates/commands/src/lib.rs
git commit -m "Keep Claude home definitions usable through existing execution paths"
```

---

### Task 4: Expose Claude MCP sources consistently in management surfaces and finish with regression evidence

**Files:**
- Modify: `rust/crates/commands/src/lib.rs`
- Test: `rust/crates/commands/src/lib.rs`
- Test: `rust/crates/runtime/src/config.rs`

- [ ] **Step 1: Add a failing `/mcp help` source-string test for Claude config files**

```rust
#[test]
fn mcp_usage_mentions_claude_settings_sources() {
    let help = render_mcp_usage(None);
    assert!(help.contains("~/.claude/settings.json"));
    assert!(help.contains("CLAUDE_CONFIG_DIR/settings.json"));
}
```

- [ ] **Step 2: Run the targeted help test to verify it fails first**

Run: `cd rust && cargo test -p commands mcp_usage_mentions_claude_settings_sources -- --nocapture`
Expected: FAIL because the help text currently only lists `.claw/settings.json` and `.claw/settings.local.json`.

- [ ] **Step 3: Update MCP help/report strings to describe the merged Claude user sources**

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

- [ ] **Step 4: Add a merged-report test showing Claude MCP servers through `/mcp show`**

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

- [ ] **Step 5: Run the full targeted regression commands for this feature set**

Run: `cd rust && cargo test -p runtime config -- --nocapture && cargo test -p commands -- --nocapture && cargo test -p tools -- --nocapture && cargo check -p runtime -p commands -p tools -p rusty-claude-cli --quiet && git diff --check`
Expected: PASS with Claude home discovery, usability, and MCP reporting covered and no formatting/check regressions.

- [ ] **Step 6: Commit the final management-surface and regression update**

```bash
git add rust/crates/runtime/src/config.rs rust/crates/commands/src/lib.rs rust/crates/tools/src/lib.rs
git commit -m "Finish Claude home compatibility for discovery, usability, and MCP reporting"
```

---

## Self-review checklist

- [ ] Spec coverage confirmed:
  - `~/.claude/skills` discovery + usability covered by Tasks 2 and 3
  - `~/.claude/agents` discovery + real execution-path coverage covered by Tasks 2 and 3
  - `~/.claude/commands` legacy-command compatibility covered by Tasks 2 and 3
  - `~/.claude/settings.json` / `CLAUDE_CONFIG_DIR/settings.json` MCP merge covered by Tasks 1 and 4
  - plugins explicitly deferred and not accidentally pulled into the task list
- [ ] Placeholder scan complete: no TBD/TODO/"implement later" text remains
- [ ] Type consistency checked:
  - `ConfigLoader::discover()` remains the single discovery source for config entries
  - `resolve_skill_path()` and compat root helpers use the same `~/.claude` directories as `discover_skill_roots()`
  - agent execution continues to use `execute_agent_with_spawn()` rather than inventing a new runtime path

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-18-claude-home-compatibility-implementation-plan.md`. Two execution options:

1. **Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration
2. **Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
