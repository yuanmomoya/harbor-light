use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::status::LightState;

use super::{activity_id, HookAction, HookProvider, NormalizedActivity, Provider};

pub const CURSOR: CursorProvider = CursorProvider;

/// Events installed in the user-level Cursor hook configuration.
/// Start/stop events drive state. Thought and tool-start events only refresh
/// an existing lease; only explicit approval fields may switch to waiting.
pub const HOOK_EVENTS: &[&str] = &[
    "beforeSubmitPrompt",
    "preToolUse",
    "beforeShellExecution",
    "afterShellExecution",
    "beforeMCPExecution",
    "afterMCPExecution",
    "afterAgentThought",
    "stop",
    "sessionEnd",
];

pub struct CursorProvider;

#[derive(Debug, Deserialize)]
struct CursorHookPayload {
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    generation_id: Option<String>,
    #[serde(default)]
    workspace_roots: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(
        default,
        alias = "approvalRequired",
        alias = "requiresApproval",
        alias = "requires_approval"
    )]
    approval_required: Option<bool>,
    #[serde(default)]
    permission: Option<String>,
}

impl CursorHookPayload {
    fn activity_id(&self) -> Option<String> {
        activity_id(
            self.conversation_id.as_deref(),
            self.session_id.as_deref(),
            self.generation_id.as_deref(),
            self.workspace_roots
                .first()
                .map(String::as_str)
                .or(self.cwd.as_deref()),
        )
    }

    fn conversation_id(&self) -> Option<String> {
        self.conversation_id
            .clone()
            .or_else(|| self.session_id.clone())
    }

    fn approval_requested(&self) -> bool {
        self.approval_required == Some(true)
            || self.permission.as_deref().is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "ask" | "prompt" | "required"
                )
            })
    }

    fn normalized_workspace_roots(&self) -> Vec<String> {
        if self.workspace_roots.is_empty() {
            self.cwd.clone().into_iter().collect()
        } else {
            self.workspace_roots.clone()
        }
    }
}

impl HookProvider for CursorProvider {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    fn normalize(&self, payload: Value) -> Result<HookAction> {
        let payload: CursorHookPayload = serde_json::from_value(payload)?;
        let Some(event) = payload.hook_event_name.clone() else {
            return Ok(HookAction::Ignore {
                event: None,
                reason: "missing hook_event_name",
            });
        };

        if event == "sessionEnd" {
            let Some(conversation_id) = payload.conversation_id() else {
                return Ok(HookAction::Ignore {
                    event: Some(event),
                    reason: "Cursor sessionEnd missing conversation id",
                });
            };
            return Ok(HookAction::RemoveConversation {
                provider: self.provider(),
                conversation_id,
                event,
            });
        }

        let Some(activity_id) = payload.activity_id() else {
            return Ok(HookAction::Ignore {
                event: Some(event),
                reason: "Cursor event missing activity id",
            });
        };

        if matches!(event.as_str(), "permissionRequest" | "PermissionRequest")
            || payload.approval_requested()
        {
            return Ok(HookAction::Upsert(NormalizedActivity {
                provider: self.provider(),
                activity_id,
                conversation_id: payload.conversation_id(),
                generation_id: payload.generation_id.clone(),
                workspace_roots: payload.normalized_workspace_roots(),
                state: LightState::Waiting,
                event,
            }));
        }

        if matches!(
            event.as_str(),
            "preToolUse" | "beforeShellExecution" | "beforeMCPExecution" | "afterAgentThought"
        ) {
            return Ok(HookAction::Touch {
                provider: self.provider(),
                activity_id,
                generation_id: payload.generation_id.clone(),
                workspace_roots: payload.normalized_workspace_roots(),
                event,
            });
        }

        if matches!(event.as_str(), "afterShellExecution" | "afterMCPExecution") {
            return Ok(HookAction::SetExisting {
                provider: self.provider(),
                activity_id,
                generation_id: payload.generation_id.clone(),
                workspace_roots: payload.normalized_workspace_roots(),
                state: LightState::Working,
                event,
            });
        }

        let state = match event.as_str() {
            "beforeSubmitPrompt" => LightState::Working,
            "stop" => match payload.status.as_deref() {
                Some("error") => LightState::Waiting,
                Some("completed" | "aborted") | None => LightState::Done,
                Some(_) => LightState::Done,
            },
            _ => {
                return Ok(HookAction::Ignore {
                    event: Some(event),
                    reason: "unsupported Cursor event",
                })
            }
        };

        Ok(HookAction::Upsert(NormalizedActivity {
            provider: self.provider(),
            activity_id,
            conversation_id: payload.conversation_id(),
            generation_id: payload.generation_id.clone(),
            workspace_roots: payload.normalized_workspace_roots(),
            state,
            event: if let Some(reason) = payload.reason.filter(|value| !value.is_empty()) {
                format!("{event}:{reason}")
            } else {
                event
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_starts_conversation_activity() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "beforeSubmitPrompt",
                "conversation_id": "chat-1",
                "generation_id": "turn-2",
                "workspace_roots": ["/tmp/project"]
            }))
            .unwrap();
        let HookAction::Upsert(activity) = action else {
            panic!("expected upsert");
        };
        assert_eq!(activity.activity_id, "chat-1");
        assert_eq!(activity.generation_id.as_deref(), Some("turn-2"));
        assert_eq!(activity.state, LightState::Working);
    }

    #[test]
    fn stop_distinguishes_error_and_abort() {
        for (status, expected) in [
            ("completed", LightState::Done),
            ("aborted", LightState::Done),
            ("error", LightState::Waiting),
        ] {
            let action = CURSOR
                .normalize(serde_json::json!({
                    "hook_event_name": "stop",
                    "conversation_id": "chat-1",
                    "generation_id": "turn-2",
                    "status": status
                }))
                .unwrap();
            let HookAction::Upsert(activity) = action else {
                panic!("expected upsert");
            };
            assert_eq!(activity.state, expected);
        }
    }

    #[test]
    fn session_end_removes_whole_conversation() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "sessionEnd",
                "session_id": "chat-1",
                "reason": "window_close"
            }))
            .unwrap();
        assert!(matches!(
            action,
            HookAction::RemoveConversation {
                provider: Provider::Cursor,
                ref conversation_id,
                ..
            } if conversation_id == "chat-1"
        ));
    }

    #[test]
    fn heartbeat_does_not_create_a_new_activity() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "preToolUse",
                "conversation_id": "chat-1"
            }))
            .unwrap();
        assert!(matches!(action, HookAction::Touch { .. }));
    }

    #[test]
    fn shell_start_is_only_a_heartbeat_regardless_of_sandbox() {
        for sandbox in [false, true] {
            let action = CURSOR
                .normalize(serde_json::json!({
                    "hook_event_name": "beforeShellExecution",
                    "conversation_id": "chat-1",
                    "command": "npm install",
                    "sandbox": sandbox
                }))
                .unwrap();
            assert!(matches!(action, HookAction::Touch { .. }));
        }
    }

    #[test]
    fn explicit_approval_request_marks_conversation_waiting() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "preToolUse",
                "conversation_id": "chat-1",
                "approval_required": true
            }))
            .unwrap();
        let HookAction::Upsert(activity) = action else {
            panic!("expected waiting upsert");
        };
        assert_eq!(activity.state, LightState::Waiting);
    }

    #[test]
    fn thought_is_only_a_heartbeat() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "afterAgentThought",
                "conversation_id": "chat-1",
                "text": "long reasoning that must not create a new task"
            }))
            .unwrap();
        assert!(matches!(action, HookAction::Touch { .. }));
    }

    #[test]
    fn shell_completion_updates_existing_activity_only() {
        let action = CURSOR
            .normalize(serde_json::json!({
                "hook_event_name": "afterShellExecution",
                "conversation_id": "chat-1",
                "exit_code": 0
            }))
            .unwrap();
        let HookAction::SetExisting {
            ref activity_id,
            state,
            ..
        } = action
        else {
            panic!("expected set-existing, got {action:?}");
        };
        assert_eq!(activity_id, "chat-1");
        assert_eq!(state, LightState::Working);
    }
}
