use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::status::LightState;

use super::{activity_id, HookAction, HookProvider, NormalizedActivity, Provider};

pub const CODEX: CodexProvider = CodexProvider;

pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "PermissionRequest",
    "SessionEnd",
];

pub struct CodexProvider;

#[derive(Debug, Deserialize)]
struct CodexHookPayload {
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

impl HookProvider for CodexProvider {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn normalize(&self, payload: Value) -> Result<HookAction> {
        let payload: CodexHookPayload = serde_json::from_value(payload)?;
        let Some(event) = payload.hook_event_name else {
            return Ok(HookAction::Ignore {
                event: None,
                reason: "missing hook_event_name",
            });
        };
        let state = match event.as_str() {
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => {
                LightState::Working
            }
            "PermissionRequest" => LightState::Waiting,
            "Stop" | "SessionEnd" => LightState::Done,
            _ => {
                return Ok(HookAction::Ignore {
                    event: Some(event),
                    reason: "unsupported Codex event",
                })
            }
        };
        let Some(activity_id) = activity_id(
            None,
            payload.session_id.as_deref(),
            None,
            payload.cwd.as_deref(),
        ) else {
            return Ok(HookAction::Ignore {
                event: Some(event),
                reason: "missing session_id and cwd",
            });
        };

        Ok(HookAction::Upsert(NormalizedActivity {
            provider: self.provider(),
            activity_id,
            conversation_id: payload.session_id,
            generation_id: None,
            workspace_roots: payload.cwd.into_iter().collect(),
            state,
            event,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_permission_request() {
        let action = CODEX
            .normalize(serde_json::json!({
                "hook_event_name": "PermissionRequest",
                "session_id": "s1",
                "cwd": "/tmp/project"
            }))
            .unwrap();
        let HookAction::Upsert(activity) = action else {
            panic!("expected upsert");
        };
        assert_eq!(activity.provider, Provider::Codex);
        assert_eq!(activity.activity_id, "s1");
        assert_eq!(activity.state, LightState::Waiting);
    }

    #[test]
    fn post_tool_use_returns_to_working_after_approval() {
        let action = CODEX
            .normalize(serde_json::json!({
                "hook_event_name": "PostToolUse",
                "session_id": "s1",
                "cwd": "/tmp/project"
            }))
            .unwrap();
        let HookAction::Upsert(activity) = action else {
            panic!("expected upsert");
        };
        assert_eq!(activity.state, LightState::Working);
    }
}
