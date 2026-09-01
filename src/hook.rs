use std::io::{Read, Write};

use anyhow::Result;

use crate::activity::apply_hook_action;
use crate::paths::{append_log, Paths};
use crate::providers::{adapter, HookAction, Provider};
use crate::status::{write_current, StatusSnapshot};

/// Read one provider hook payload from `stdin` and update its independent
/// activity file.
///
/// Always returns `Ok(())` after logging so a monitoring failure never blocks
/// the coding agent. Stdout stays empty because it is part of hook protocols.
pub fn run(paths: &Paths, provider: Provider, mut stdin: impl Read) -> Result<()> {
    let mut raw = String::new();
    if let Err(err) = stdin.read_to_string(&mut raw) {
        append_log(paths, &format!("{provider} hook stdin read failed: {err}"));
        return Ok(());
    }

    // Cursor on Windows may prefix hook stdin with a UTF-8 BOM. Rust's
    // `str::trim` does not remove U+FEFF, so strip it explicitly before
    // handing the payload to serde_json.
    let trimmed = raw.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        append_log(paths, &format!("{provider} hook received empty stdin"));
        return Ok(());
    }

    let payload = match serde_json::from_str(trimmed) {
        Ok(payload) => payload,
        Err(err) => match salvage_hook_payload(trimmed) {
            Some(payload) => {
                append_log(
                    paths,
                    &format!("{provider} hook salvaged truncated json after parse error: {err}"),
                );
                payload
            }
            None => {
                append_log(paths, &format!("{provider} hook json parse failed: {err}"));
                return Ok(());
            }
        },
    };

    let action = match adapter(provider).normalize(payload) {
        Ok(action) => action,
        Err(err) => {
            append_log(
                paths,
                &format!("{provider} hook normalization failed: {err}"),
            );
            return Ok(());
        }
    };

    if let HookAction::Ignore { event, reason } = &action {
        append_log(
            paths,
            &format!("{provider} hook ignored event={event:?} reason={reason}"),
        );
        return Ok(());
    }

    let action_summary = format!("{action:?}");
    match apply_hook_action(paths, action) {
        Ok(snapshot) => {
            // Keep the old Codex status file working for existing scripts and
            // installations while the UI uses the multi-activity store.
            if provider == Provider::Codex {
                if let Some(activity) = snapshot.as_ref() {
                    let legacy = StatusSnapshot::new(
                        activity.state,
                        "hook",
                        Some(activity.event.clone()),
                        activity.conversation_id.clone(),
                    );
                    if let Err(err) = write_current(paths, &legacy) {
                        append_log(paths, &format!("Codex legacy status write failed: {err}"));
                    }
                }
            }
            append_log(paths, &format!("{provider} hook applied {action_summary}"));
        }
        Err(err) => append_log(
            paths,
            &format!("{provider} hook activity write failed: {err}"),
        ),
    }

    let _ = std::io::stdout().flush();
    Ok(())
}

fn salvage_hook_payload(raw: &str) -> Option<serde_json::Value> {
    let hook_event_name = extract_json_string(raw, "hook_event_name")?;
    let mut payload = serde_json::json!({ "hook_event_name": hook_event_name });
    for key in [
        "conversation_id",
        "session_id",
        "generation_id",
        "status",
        "cwd",
        "permission",
    ] {
        if let Some(value) = extract_json_string(raw, key) {
            payload[key] = serde_json::Value::String(value);
        }
    }
    if let Some(value) = extract_json_bool(raw, "approval_required")
        .or_else(|| extract_json_bool(raw, "approvalRequired"))
        .or_else(|| extract_json_bool(raw, "requiresApproval"))
    {
        payload["approval_required"] = serde_json::Value::Bool(value);
    }
    Some(payload)
}

fn extract_json_string(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut search = raw;
    while let Some(idx) = search.find(&needle) {
        let after = search[idx + needle.len()..].trim_start();
        if let Some(after) = after.strip_prefix(':') {
            if let Some(value) = parse_json_string(after.trim_start()) {
                return Some(value);
            }
        }
        search = &search[idx + needle.len()..];
    }
    None
}

fn extract_json_bool(raw: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let idx = raw.find(&needle)?;
    let after = raw[idx + needle.len()..].trim_start().strip_prefix(':')?;
    let after = after.trim_start();
    if after.starts_with("true") {
        Some(true)
    } else if after.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn parse_json_string(input: &str) -> Option<String> {
    let mut chars = input.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        hex.push(chars.next()?);
                    }
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                }
                other => out.push(other),
            },
            ch if ch.is_control() => return None,
            ch => out.push(ch),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tempfile::tempdir;

    use super::*;
    use crate::activity::{read_activities, resolve_display_state, ProviderResets};
    use crate::status::{read_current, DisplayState, LightState};

    fn paths_in_tmp() -> (tempfile::TempDir, Paths) {
        let dir = tempdir().unwrap();
        let paths = Paths {
            home: dir.path().to_path_buf(),
            codex_home: None,
        };
        (dir, paths)
    }

    #[test]
    fn codex_hook_writes_activity_and_legacy_status() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Codex,
            Cursor::new(
                r#"{"hook_event_name":"PermissionRequest","session_id":"s1","cwd":"/tmp"}"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].provider, Provider::Codex);
        assert_eq!(activities[0].state, LightState::Waiting);
        let legacy = read_current(&paths).unwrap().unwrap();
        assert_eq!(legacy.state, LightState::Waiting);
    }

    #[test]
    fn cursor_conversations_do_not_overwrite_each_other_or_legacy_status() {
        let (_dir, paths) = paths_in_tmp();
        for id in ["chat-a", "chat-b"] {
            run(
                &paths,
                Provider::Cursor,
                Cursor::new(format!(
                    r#"{{"hook_event_name":"beforeSubmitPrompt","conversation_id":"{id}","generation_id":"turn-1"}}"#
                )),
            )
            .unwrap();
        }

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 2);
        assert!(activities
            .iter()
            .all(|activity| activity.provider == Provider::Cursor));
        assert!(read_current(&paths).unwrap().is_none());
    }

    #[test]
    fn cursor_hook_accepts_utf8_bom() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                "\u{feff}{\"hook_event_name\":\"beforeSubmitPrompt\",\"conversation_id\":\"bom-chat\"}",
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].provider, Provider::Cursor);
        assert_eq!(activities[0].activity_id, "bom-chat");
        assert_eq!(activities[0].state, LightState::Working);
    }

    #[test]
    fn cursor_abort_finishes_only_its_conversation() {
        let (_dir, paths) = paths_in_tmp();
        for id in ["chat-a", "chat-b"] {
            run(
                &paths,
                Provider::Cursor,
                Cursor::new(format!(
                    r#"{{"hook_event_name":"beforeSubmitPrompt","conversation_id":"{id}"}}"#
                )),
            )
            .unwrap();
        }
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"stop","conversation_id":"chat-a","status":"aborted"}"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 2);
        let chat_a = activities
            .iter()
            .find(|activity| activity.activity_id == "chat-a")
            .unwrap();
        assert_eq!(chat_a.state, LightState::Done);
        assert_eq!(
            activities
                .iter()
                .find(|activity| activity.activity_id == "chat-b")
                .unwrap()
                .state,
            LightState::Working
        );
    }

    #[test]
    fn cursor_approval_and_other_worker_display_both_lamps() {
        let (_dir, paths) = paths_in_tmp();
        for id in ["waiting-chat", "working-chat"] {
            run(
                &paths,
                Provider::Cursor,
                Cursor::new(format!(
                    r#"{{"hook_event_name":"beforeSubmitPrompt","conversation_id":"{id}"}}"#
                )),
            )
            .unwrap();
        }
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"preToolUse","conversation_id":"waiting-chat","approval_required":true}"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(
            resolve_display_state(None, &activities, &[], &ProviderResets::new()),
            DisplayState::WAITING_AND_WORKING
        );
    }

    #[test]
    fn malformed_and_unknown_payloads_do_not_fail() {
        let (_dir, paths) = paths_in_tmp();
        run(&paths, Provider::Cursor, Cursor::new("")).unwrap();
        run(&paths, Provider::Cursor, Cursor::new("not-json")).unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(r#"{"hook_event_name":"workspaceOpen"}"#),
        )
        .unwrap();
        assert!(read_activities(&paths).unwrap().is_empty());
    }

    #[test]
    fn cursor_late_thought_does_not_revive_stopped_conversation() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"beforeSubmitPrompt","conversation_id":"chat-a"}"#,
            ),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(r#"{"hook_event_name":"stop","conversation_id":"chat-a","status":"completed"}"#),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"afterAgentThought","conversation_id":"chat-a","text":"late"}"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].state, LightState::Done);
        assert_eq!(
            resolve_display_state(None, &activities, &[], &ProviderResets::new()),
            DisplayState::DONE
        );
    }

    #[test]
    fn cursor_late_thought_does_not_recreate_after_session_end() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"beforeSubmitPrompt","conversation_id":"chat-a"}"#,
            ),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(r#"{"hook_event_name":"sessionEnd","conversation_id":"chat-a"}"#),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"afterAgentThought","conversation_id":"chat-a"}"#,
            ),
        )
        .unwrap();
        assert!(read_activities(&paths).unwrap().is_empty());
    }

    #[test]
    fn cursor_shell_completion_does_not_create_activity() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"afterShellExecution","conversation_id":"chat-a","exit_code":0}"#,
            ),
        )
        .unwrap();
        assert!(read_activities(&paths).unwrap().is_empty());
    }

    #[test]
    fn cursor_shell_completion_returns_waiting_conversation_to_working() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"beforeSubmitPrompt","conversation_id":"chat-a"}"#,
            ),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"preToolUse","conversation_id":"chat-a","approval_required":true}"#,
            ),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"afterShellExecution","conversation_id":"chat-a","exit_code":0}"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].state, LightState::Working);
    }

    #[test]
    fn salvages_truncated_cursor_thought_payload() {
        let (_dir, paths) = paths_in_tmp();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"hook_event_name":"beforeSubmitPrompt","conversation_id":"chat-a"}"#,
            ),
        )
        .unwrap();
        run(
            &paths,
            Provider::Cursor,
            Cursor::new(
                r#"{"conversation_id":"chat-a","hook_event_name":"afterAgentThought","text":"unterminated"#,
            ),
        )
        .unwrap();

        let activities = read_activities(&paths).unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].state, LightState::Working);
        assert_eq!(activities[0].event, "afterAgentThought");
    }
}
