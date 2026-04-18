use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::omc_compat::normalize_mode_name;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OmcLifecycleEvent {
    UserPromptSubmit,
    SessionStart,
    Stop,
}

impl OmcLifecycleEvent {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::Stop => "Stop",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OmcLifecycleStateBridge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl OmcLifecycleStateBridge {
    #[must_use]
    pub fn new(session_id: Option<&str>, mode: Option<&str>) -> Self {
        Self {
            session_id: normalize_optional(session_id),
            mode: mode.and_then(normalize_mode),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OmcLifecyclePayload {
    pub event: OmcLifecycleEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl OmcLifecyclePayload {
    #[must_use]
    pub fn new(
        event: OmcLifecycleEvent,
        session_id: Option<&str>,
        mode: Option<&str>,
        message: Option<&str>,
    ) -> Self {
        Self {
            event,
            session_id: normalize_optional(session_id),
            mode: mode.and_then(normalize_mode),
            message: normalize_optional(message),
        }
    }

    #[must_use]
    pub fn from_state_bridge(
        event: OmcLifecycleEvent,
        bridge: OmcLifecycleStateBridge,
        message: Option<&str>,
    ) -> Self {
        Self {
            event,
            session_id: bridge.session_id,
            mode: bridge.mode,
            message: normalize_optional(message),
        }
    }

    #[must_use]
    pub fn hook_payload(&self) -> Value {
        json!({
            "hook_event_name": self.event.as_str(),
            "event": self.event.as_str(),
            "session_id": self.session_id,
            "mode": self.mode,
            "message": self.message,
        })
    }

    #[must_use]
    pub fn hook_env_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();

        if let Some(session_id) = &self.session_id {
            pairs.push(("HOOK_SESSION_ID", session_id.clone()));
        }
        if let Some(mode) = &self.mode {
            pairs.push(("HOOK_MODE", mode.clone()));
        }
        if let Some(message) = &self.message {
            pairs.push(("HOOK_MESSAGE", message.clone()));
        }

        pairs
    }
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_mode(value: &str) -> Option<String> {
    let normalized = normalize_mode_name(value);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OmcLifecycleEvent, OmcLifecyclePayload, OmcLifecycleStateBridge,
    };
    use serde_json::json;

    #[test]
    fn lifecycle_state_bridge_normalizes_session_and_mode() {
        let bridge =
            OmcLifecycleStateBridge::new(Some(" session-123 "), Some(" Deep Interview "));

        assert_eq!(
            bridge,
            OmcLifecycleStateBridge {
                session_id: Some("session-123".to_string()),
                mode: Some("deep-interview".to_string()),
            }
        );
    }

    #[test]
    fn lifecycle_payload_from_state_bridge_serializes_hook_payload() {
        let payload = OmcLifecyclePayload::from_state_bridge(
            OmcLifecycleEvent::UserPromptSubmit,
            OmcLifecycleStateBridge::new(Some(" session-123 "), Some(" deep_interview ")),
            Some("  hello world  "),
        );

        assert_eq!(
            payload,
            OmcLifecyclePayload {
                event: OmcLifecycleEvent::UserPromptSubmit,
                session_id: Some("session-123".to_string()),
                mode: Some("deep-interview".to_string()),
                message: Some("hello world".to_string()),
            }
        );
        assert_eq!(
            payload.hook_payload(),
            json!({
                "hook_event_name": "UserPromptSubmit",
                "event": "UserPromptSubmit",
                "session_id": "session-123",
                "mode": "deep-interview",
                "message": "hello world",
            })
        );
    }

    #[test]
    fn lifecycle_payload_exposes_hook_environment_pairs() {
        let payload = OmcLifecyclePayload::new(
            OmcLifecycleEvent::Stop,
            Some(" session-456 "),
            Some(" swarm "),
            Some("  abort requested "),
        );

        assert_eq!(
            payload.hook_env_pairs(),
            vec![
                ("HOOK_SESSION_ID", "session-456".to_string()),
                ("HOOK_MODE", "team".to_string()),
                ("HOOK_MESSAGE", "abort requested".to_string()),
            ]
        );
    }
}
