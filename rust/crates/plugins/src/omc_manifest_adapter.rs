use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PluginManifestValidationError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmcManifestCompat {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: Value,
}

impl OmcManifestCompat {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.agents.is_empty()
            && self.commands.is_empty()
            && self.mcp_servers.is_null()
    }
}

pub fn adapt_omc_manifest_fields(
    raw_manifest: Value,
) -> Result<(Value, Option<OmcManifestCompat>), Vec<PluginManifestValidationError>> {
    let mut compat = OmcManifestCompat::default();
    let Value::Object(mut root) = raw_manifest else {
        return Ok((raw_manifest, None));
    };

    let mut errors = Vec::new();

    if let Some(value) = root.remove("skills") {
        match parse_string_list_field(&value, "skills") {
            Ok(values) => compat.skills = values,
            Err(error) => errors.push(error),
        }
    }

    if let Some(value) = root.remove("agents") {
        match parse_string_list_field(&value, "agents") {
            Ok(values) => compat.agents = values,
            Err(error) => errors.push(error),
        }
    }

    if let Some(value) = root.remove("mcpServers") {
        compat.mcp_servers = value;
    }

    if let Some(value) = root.remove("commands") {
        match adapt_commands_field(value) {
            Ok((commands, replacement)) => {
                compat.commands = commands;
                if let Some(replacement) = replacement {
                    root.insert("commands".to_string(), replacement);
                }
            }
            Err(error) => errors.push(error),
        }
    }

    if errors.is_empty() {
        Ok((
            Value::Object(root),
            (!compat.is_empty()).then_some(compat),
        ))
    } else {
        Err(errors)
    }
}

fn parse_string_list_field(
    value: &Value,
    field: &str,
) -> Result<Vec<String>, PluginManifestValidationError> {
    match value {
        Value::String(path) => Ok(vec![path.trim().to_string()]),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| match entry {
                Value::String(path) => Ok(path.trim().to_string()),
                _ => Err(PluginManifestValidationError::UnsupportedManifestContract {
                    detail: format!(
                        "plugin manifest field `{field}` must be a string or an array of strings for OMC compatibility.",
                    ),
                }),
            })
            .collect(),
        _ => Err(PluginManifestValidationError::UnsupportedManifestContract {
            detail: format!(
                "plugin manifest field `{field}` must be a string or an array of strings for OMC compatibility.",
            ),
        }),
    }
}

fn adapt_commands_field(
    value: Value,
) -> Result<(Vec<String>, Option<Value>), PluginManifestValidationError> {
    match value {
        Value::Array(entries) if entries.iter().all(Value::is_string) => Ok((
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(|entry| entry.trim().to_string())
                .collect(),
            Some(Value::Array(Vec::new())),
        )),
        Value::Array(entries) if entries.iter().all(Value::is_object) => {
            Ok((Vec::new(), Some(Value::Array(entries))))
        }
        Value::Array(_) => Err(PluginManifestValidationError::UnsupportedManifestContract {
            detail:
                "plugin manifest field `commands` must be either Claude Code-style string globs or claw command objects, not a mixed array."
                    .to_string(),
        }),
        other => Err(PluginManifestValidationError::UnsupportedManifestContract {
            detail: format!(
                "plugin manifest field `commands` must be an array for compatibility, got {}.",
                value_type_name(&other)
            ),
        }),
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::{adapt_omc_manifest_fields, OmcManifestCompat};
    use serde_json::json;

    #[test]
    fn extracts_omc_compat_fields_and_sanitizes_manifest() {
        let raw = json!({
            "name": "omc-plugin",
            "version": "1.0.0",
            "description": "compat manifest",
            "skills": "./skills",
            "agents": ["agents/*.md"],
            "commands": ["commands/**/*.md"],
            "mcpServers": {"demo": {"command": "uvx", "args": ["demo"]}},
        });

        let (adapted, compat) = adapt_omc_manifest_fields(raw).expect("compat adapter should pass");

        assert_eq!(
            compat,
            Some(OmcManifestCompat {
                skills: vec!["./skills".to_string()],
                agents: vec!["agents/*.md".to_string()],
                commands: vec!["commands/**/*.md".to_string()],
                mcp_servers: json!({"demo": {"command": "uvx", "args": ["demo"]}}),
            })
        );
        assert_eq!(
            adapted,
            json!({
                "name": "omc-plugin",
                "version": "1.0.0",
                "description": "compat manifest",
                "commands": [],
            })
        );
    }

    #[test]
    fn preserves_native_command_objects() {
        let raw = json!({
            "name": "native-plugin",
            "version": "1.0.0",
            "description": "native manifest",
            "commands": [{
                "name": "sync",
                "description": "sync state",
                "command": "scripts/sync.sh"
            }],
        });

        let (adapted, compat) = adapt_omc_manifest_fields(raw.clone()).expect("native commands should stay intact");

        assert_eq!(adapted, raw);
        assert_eq!(compat, None);
    }
}
