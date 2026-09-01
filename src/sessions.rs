use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::status::LightState;

/// Last-resort lease for a working session when Codex disappears without a
/// terminal event (power loss, crash before AppKit notification, corrupt log).
/// Normal quits are reset immediately by the workspace termination observer.
pub const WORKING_IDLE_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// One Codex rollout session and the light state inferred from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub path: PathBuf,
    pub session_id: Option<String>,
    pub state: LightState,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// Scan recent rollout JSONL files and return per-session states.
///
/// Only task lifecycle and explicit user-attention events are treated as
/// stable signals. A manually stopped response ends with `turn_aborted`
/// instead of `task_complete`.
pub fn scan_sessions(sessions_dir: &Path) -> Vec<SessionSnapshot> {
    let mut files = recent_rollout_files(sessions_dir);
    files.sort_by_key(|entry| Reverse(entry.1));
    files.truncate(24);

    files
        .into_iter()
        .filter_map(|(path, _)| parse_rollout(&path))
        .collect()
}

#[cfg(test)]
pub fn aggregate_sessions(sessions: &[SessionSnapshot]) -> LightState {
    crate::status::aggregate_states(sessions.iter().map(|s| s.state))
}

/// Combine the latest hook snapshot with rollout-file sessions.
/// Hooks win on `waiting` (JSONL does not record permission prompts).
#[cfg(test)]
pub fn resolve_display_state(
    hook: Option<&crate::status::StatusSnapshot>,
    sessions: &[SessionSnapshot],
) -> LightState {
    resolve_display_state_after(hook, sessions, None)
}

/// Resolve the aggregate state, optionally ignoring active states recorded
/// before a known Codex application termination.
#[cfg(test)]
pub fn resolve_display_state_after(
    hook: Option<&crate::status::StatusSnapshot>,
    sessions: &[SessionSnapshot],
    activity_reset_at: Option<DateTime<Utc>>,
) -> LightState {
    use crate::status::{aggregate_states, apply_done_timeout};

    let now = Utc::now();
    let session_state = aggregate_states(sessions.iter().map(|session| {
        effective_active_state(session.state, session.updated_at, activity_reset_at, now)
    }));
    let mut states = vec![session_state];
    if let Some(hook) = hook {
        if hook.age() < chrono::Duration::hours(2) {
            states.push(effective_active_state(
                hook.state,
                hook.updated_at,
                activity_reset_at,
                now,
            ));
        }
    }
    let aggregated = aggregate_states(states);
    if aggregated != LightState::Done {
        return aggregated;
    }
    let mut newest_done: Option<chrono::DateTime<chrono::Utc>> = None;
    if let Some(hook) = hook.filter(|h| h.state == LightState::Done) {
        newest_done = Some(hook.updated_at);
    }
    for session in sessions {
        if session.state == LightState::Done {
            newest_done = Some(match newest_done {
                Some(ts) => ts.max(session.updated_at),
                None => session.updated_at,
            });
        }
    }
    apply_done_timeout(LightState::Done, newest_done)
}

#[cfg(test)]
fn effective_active_state(
    state: LightState,
    updated_at: DateTime<Utc>,
    activity_reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> LightState {
    let is_active = matches!(state, LightState::Working | LightState::Waiting);
    if is_active && activity_reset_at.is_some_and(|reset_at| updated_at <= reset_at) {
        return LightState::Idle;
    }

    let stale_after = chrono::Duration::from_std(WORKING_IDLE_AFTER).unwrap();
    if state == LightState::Working && now - updated_at >= stale_after {
        return LightState::Idle;
    }

    state
}

fn recent_rollout_files(sessions_dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    if !sessions_dir.exists() {
        return out;
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(36 * 3600))
        .unwrap_or(std::time::UNIX_EPOCH);

    let Ok(year_dirs) = fs::read_dir(sessions_dir) else {
        return out;
    };
    for year in year_dirs.flatten() {
        let year_path = year.path();
        if !year_path.is_dir() {
            continue;
        }
        let Ok(month_dirs) = fs::read_dir(&year_path) else {
            continue;
        };
        for month in month_dirs.flatten() {
            let month_path = month.path();
            if !month_path.is_dir() {
                continue;
            }
            let Ok(day_dirs) = fs::read_dir(&month_path) else {
                continue;
            };
            for day in day_dirs.flatten() {
                let day_path = day.path();
                if !day_path.is_dir() {
                    continue;
                }
                let Ok(entries) = fs::read_dir(&day_path) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                        continue;
                    }
                    let Ok(meta) = entry.metadata() else {
                        continue;
                    };
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    if mtime >= cutoff {
                        out.push((path, mtime));
                    }
                }
            }
        }
    }
    out
}

pub fn parse_rollout(path: &Path) -> Option<SessionSnapshot> {
    let raw = fs::read_to_string(path).ok()?;
    parse_rollout_text(path, &raw)
}

fn parse_rollout_text(path: &Path, raw: &str) -> Option<SessionSnapshot> {
    let mut session_id = session_id_from_filename(path);
    let mut last_task: Option<(String, DateTime<Utc>)> = None;
    let mut updated_at = file_mtime_utc(path);

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(env) = serde_json::from_str::<Envelope>(line) else {
            continue;
        };
        if let Some(ts) = env.timestamp.as_deref().and_then(parse_ts) {
            updated_at = ts;
        }
        match env.kind.as_deref() {
            Some("session_meta") => {
                if let Some(id) = env
                    .payload
                    .get("session_id")
                    .or_else(|| env.payload.get("id"))
                    .and_then(|v| v.as_str())
                {
                    session_id = Some(id.to_string());
                }
            }
            Some("event_msg") => {
                let subtype = env
                    .payload
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match subtype.as_str() {
                    "task_started"
                    | "exec_command_begin"
                    | "exec_command_end"
                    | "patch_apply_begin"
                    | "patch_apply_end"
                    | "mcp_tool_call_begin"
                    | "mcp_tool_call_end"
                    | "agent_reasoning"
                    | "agent_message"
                    | "item_completed" => {
                        last_task = Some(("task_started".into(), updated_at));
                    }
                    "exec_approval_request"
                    | "apply_patch_approval_request"
                    | "request_permissions"
                    | "request_user_input"
                    | "elicitation_request" => {
                        last_task = Some(("waiting".into(), updated_at));
                    }
                    "task_complete" | "turn_aborted" => {
                        last_task = Some((subtype, updated_at));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let state = match last_task {
        Some((ref kind, _)) if kind == "task_started" => LightState::Working,
        Some((ref kind, ts)) if kind == "waiting" => {
            updated_at = ts;
            LightState::Waiting
        }
        Some((ref kind, ts)) if kind == "task_complete" || kind == "turn_aborted" => {
            updated_at = ts;
            LightState::Done
        }
        _ => LightState::Idle,
    };

    Some(SessionSnapshot {
        path: path.to_path_buf(),
        session_id,
        state,
        updated_at,
    })
}

fn session_id_from_filename(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    // rollout-2026-08-20T03-34-00-<uuid>
    name.rsplit('-').next().map(str::to_string)
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|ts| ts.with_timezone(&Utc))
}

fn file_mtime_utc(path: &Path) -> DateTime<Utc> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use tempfile::tempdir;

    #[test]
    fn task_started_means_working() {
        let text = r#"
{"timestamp":"2026-08-20T03:00:00Z","type":"session_meta","payload":{"session_id":"abc"}}
{"timestamp":"2026-08-20T03:00:01Z","type":"event_msg","payload":{"type":"task_started"}}
"#;
        let snap = parse_rollout_text(Path::new("rollout-x-abc.jsonl"), text).unwrap();
        assert_eq!(snap.state, LightState::Working);
        assert_eq!(snap.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn complete_after_start_means_done() {
        let text = r#"
{"timestamp":"2026-08-20T03:00:00Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-08-20T03:00:05Z","type":"event_msg","payload":{"type":"token_count"}}
{"timestamp":"2026-08-20T03:00:10Z","type":"event_msg","payload":{"type":"task_complete"}}
"#;
        let snap = parse_rollout_text(Path::new("rollout.jsonl"), text).unwrap();
        assert_eq!(snap.state, LightState::Done);
    }

    #[test]
    fn abort_after_start_means_done() {
        let text = r#"
{"timestamp":"2026-08-20T03:00:00Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-08-20T03:00:05Z","type":"event_msg","payload":{"type":"token_count"}}
{"timestamp":"2026-08-20T03:00:10Z","type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}
"#;
        let snap = parse_rollout_text(Path::new("rollout.jsonl"), text).unwrap();
        assert_eq!(snap.state, LightState::Done);
    }

    #[test]
    fn ignores_unrelated_events() {
        let text = r#"
{"timestamp":"2026-08-20T03:00:00Z","type":"response_item","payload":{"type":"function_call"}}
"#;
        let snap = parse_rollout_text(Path::new("rollout.jsonl"), text).unwrap();
        assert_eq!(snap.state, LightState::Idle);
    }

    #[test]
    fn approval_request_means_waiting() {
        for subtype in [
            "exec_approval_request",
            "apply_patch_approval_request",
            "request_permissions",
        ] {
            let text = format!(
                r#"
{{"timestamp":"2026-08-20T03:00:00Z","type":"event_msg","payload":{{"type":"task_started"}}}}
{{"timestamp":"2026-08-20T03:00:05Z","type":"event_msg","payload":{{"type":"{subtype}"}}}}
"#
            );
            let snap = parse_rollout_text(Path::new("rollout.jsonl"), &text).unwrap();
            assert_eq!(snap.state, LightState::Waiting, "subtype={subtype}");
        }
    }

    #[test]
    fn tool_execution_after_approval_returns_to_working() {
        let text = r#"
{"timestamp":"2026-08-20T03:00:00Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-08-20T03:00:05Z","type":"event_msg","payload":{"type":"exec_approval_request"}}
{"timestamp":"2026-08-20T03:00:10Z","type":"event_msg","payload":{"type":"exec_command_begin"}}
"#;
        let snap = parse_rollout_text(Path::new("rollout.jsonl"), text).unwrap();
        assert_eq!(snap.state, LightState::Working);
    }

    #[test]
    fn scan_walks_date_tree() {
        let dir = tempdir().unwrap();
        let leaf = dir.path().join("2026/08/20");
        fs::create_dir_all(&leaf).unwrap();
        let file = leaf.join("rollout-2026-08-20T03-00-00-zzzz.jsonl");
        let mut f = fs::File::create(&file).unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-08-20T03:00:00Z","type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .unwrap();
        let sessions = scan_sessions(dir.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, LightState::Working);
        assert_eq!(aggregate_sessions(&sessions), LightState::Working);
    }

    #[test]
    fn waiting_hook_outranks_working_session() {
        let sessions = [SessionSnapshot {
            path: PathBuf::from("a.jsonl"),
            session_id: Some("a".into()),
            state: LightState::Working,
            updated_at: Utc::now(),
        }];
        let hook = crate::status::StatusSnapshot::new(
            LightState::Waiting,
            "hook",
            Some("PermissionRequest".into()),
            None,
        );
        assert_eq!(
            resolve_display_state(Some(&hook), &sessions),
            LightState::Waiting
        );
    }

    #[test]
    fn stale_done_becomes_idle() {
        let mut hook = crate::status::StatusSnapshot::new(LightState::Done, "hook", None, None);
        hook.updated_at = Utc::now() - chrono::Duration::seconds(10);
        assert_eq!(resolve_display_state(Some(&hook), &[]), LightState::Idle);
    }

    #[test]
    fn application_termination_resets_older_active_states() {
        let reset_at = Utc::now();
        let sessions = [SessionSnapshot {
            path: PathBuf::from("a.jsonl"),
            session_id: Some("a".into()),
            state: LightState::Working,
            updated_at: reset_at - chrono::Duration::seconds(1),
        }];
        let mut hook = crate::status::StatusSnapshot::new(
            LightState::Waiting,
            "hook",
            Some("PermissionRequest".into()),
            None,
        );
        hook.updated_at = reset_at - chrono::Duration::seconds(1);

        assert_eq!(
            resolve_display_state_after(Some(&hook), &sessions, Some(reset_at)),
            LightState::Idle
        );
    }

    #[test]
    fn activity_after_application_termination_becomes_working_again() {
        let reset_at = Utc::now() - chrono::Duration::seconds(2);
        let sessions = [SessionSnapshot {
            path: PathBuf::from("a.jsonl"),
            session_id: Some("a".into()),
            state: LightState::Working,
            updated_at: reset_at + chrono::Duration::seconds(1),
        }];

        assert_eq!(
            resolve_display_state_after(None, &sessions, Some(reset_at)),
            LightState::Working
        );
    }

    #[test]
    fn orphaned_working_session_eventually_becomes_idle() {
        let sessions = [SessionSnapshot {
            path: PathBuf::from("a.jsonl"),
            session_id: Some("a".into()),
            state: LightState::Working,
            updated_at: Utc::now()
                - chrono::Duration::from_std(WORKING_IDLE_AFTER).unwrap()
                - chrono::Duration::seconds(1),
        }];

        assert_eq!(resolve_display_state(None, &sessions), LightState::Idle);
    }
}
