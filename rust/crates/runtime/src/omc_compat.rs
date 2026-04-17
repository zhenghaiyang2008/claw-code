use serde::{Deserialize, Serialize};

pub const OMC_COMPAT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OmcCompatHandoff {
    pub handoff_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_skill: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_skill_args: Vec<String>,
}

#[must_use]
pub fn normalize_mode_name(mode: &str) -> &str {
    let trimmed = mode.trim();
    if trimmed.is_empty() {
        return trimmed;
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "deep-interview" | "deep_interview" | "deep interview" | "deepinterview" => {
            "deep-interview"
        }
        "ultrawork" | "ultra-work" | "ultra_work" | "ultra work" => "ultrawork",
        "verification"
        | "verify"
        | "verifier"
        | "verificationagent"
        | "verification-agent"
        | "verification_agent" => "verification",
        "team" | "swarm" => "team",
        "ralph" => "ralph",
        _ => trimmed,
    }
}

#[must_use]
pub fn build_omc_handoff(next_skill: Option<&str>, args: &[&str], path: &str) -> OmcCompatHandoff {
    OmcCompatHandoff {
        handoff_path: path.trim().to_string(),
        next_skill: next_skill.and_then(|value| {
            let normalized = normalize_mode_name(value);
            if normalized.is_empty() {
                None
            } else {
                Some(normalized.to_string())
            }
        }),
        next_skill_args: args
            .iter()
            .filter_map(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_omc_handoff, normalize_mode_name, OmcCompatHandoff, OMC_COMPAT_SCHEMA_VERSION,
    };

    #[test]
    fn normalizes_known_omc_mode_aliases() {
        assert_eq!(normalize_mode_name("deep_interview"), "deep-interview");
        assert_eq!(normalize_mode_name(" Deep Interview "), "deep-interview");
        assert_eq!(normalize_mode_name("swarm"), "team");
        assert_eq!(normalize_mode_name("Verifier"), "verification");
        assert_eq!(normalize_mode_name(" ultrawork "), "ultrawork");
        assert_eq!(normalize_mode_name("custom-mode"), "custom-mode");
    }

    #[test]
    fn builds_trimmed_handoff_with_normalized_skill() {
        let handoff = build_omc_handoff(
            Some(" deep_interview "),
            &["  --deliberate ", "", " next "],
            " .omx/specs/handoff.md ",
        );

        assert_eq!(
            handoff,
            OmcCompatHandoff {
                handoff_path: ".omx/specs/handoff.md".to_string(),
                next_skill: Some("deep-interview".to_string()),
                next_skill_args: vec!["--deliberate".to_string(), "next".to_string()],
            }
        );
        assert_eq!(OMC_COMPAT_SCHEMA_VERSION, 1);
    }
}
