# `~/.claude/plugins` Follow-up Design

## 背景

上一轮 `~/.claude` Home Compatibility 实现已完成：
- `~/.claude/skills`
- `~/.claude/agents`
- `~/.claude/commands`
- `~/.claude/settings.json` / `CLAUDE_CONFIG_DIR/settings.json` 中 `mcpServers`

但 `plugins` 被明确延期。本设计用于承接该延期项，确保后续实现有清晰边界，不与现有 claw 插件数据面冲突。

## 目标

在保持现有 claw plugin registry / settings / installed.json / install_path 模型不变的前提下，增加对以下用户级插件发现的只读兼容：

- `~/.claude/plugins/*/.claude-plugin/plugin.json`
- `CLAUDE_CONFIG_DIR/plugins/*/.claude-plugin/plugin.json`（如果存在）

## 非目标

本轮仍不做：
- 把 `~/.claude/plugins` 变成默认插件安装目录
- 把 `.claude` 变成插件主写入面
- 更宽松的“目录即插件根”发现
- 完整 plugin lifecycle 语义重构
- 与现有 registry 双向同步

## 设计基线

1. **只读兼容**
   - `~/.claude/plugins` 只参与发现和加载
   - 不承载 install / uninstall / enable / disable 的主写入职责

2. **项目级优先于用户级**
   - 沿用现有 claw 总优先级模型

3. **manifest 规则保持现状**
   - 只认 `*/.claude-plugin/plugin.json`
   - 不引入新的插件目录契约

## 模块边界

### Plugin Discovery Adapter
主要落点：`rust/crates/plugins/src/lib.rs`

职责：
- 将 `~/.claude/plugins/*/.claude-plugin/plugin.json` 纳入用户级插件发现
- 复用现有 `load_plugin_from_directory()` / `plugin_manifest_path()`
- 不修改现有 registry 读写路径

### Reporting / Management Surface Alignment
主要落点：`rust/crates/plugins/src/lib.rs` 与相关 CLI surface

职责：
- 列表中区分 Claude 用户级来源
- 明确 shadowing / broken plugin / discovered-only plugin 的状态
- install / update / uninstall 仍以当前 claw 数据面为准

## 验收标准

- `~/.claude/plugins/*/.claude-plugin/plugin.json` 能被发现
- 发现的插件能在当前插件列表/报告面中体现来源
- 现有 installed.json / settings.json 语义不回归
- 项目级插件仍优先于用户级 Claude 插件

## 测试方向

1. Claude 用户级 plugin discovery 测试
2. plugin report / list 中来源与状态展示测试
3. project-over-user precedence 回归测试
4. registry path / settings path 不被改写的回归测试
