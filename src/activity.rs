use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::providers::{HookAction, NormalizedActivity, Provider};
use crate::sessions::{SessionSnapshot, WORKING_IDLE_AFTER};
use crate::status::{apply_done_timeout, DisplayState, LightState, StatusSnapshot};

pub type ProviderResets = BTreeMap<Provider, DateTime<Utc>>;

/// One independently tracked agent conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub provider: Provider,
    pub activity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<String>,
    pub state: LightState,
    pub updated_at: DateTime<Utc>,
    pub source: String,
    pub event: String,
}

impl From<NormalizedActivity> for ActivitySnapshot {
    fn from(activity: NormalizedActivity) -> Self {
        Self {
            provider: activity.provider,
            activity_id: activity.activity_id,
            conversation_id: activity.conversation_id,
            generation_id: activity.generation_id,
            workspace_roots: activity.workspace_roots,
            state: activity.state,
            updated_at: Utc::now(),
            source: "hook".into(),
            event: activity.event,
        }
    }
}

pub fn apply_hook_action(paths: &Paths, action: HookAction) -> Result<Option<ActivitySnapshot>> {
    match action {
        HookAction::Upsert(activity) => {
            let snapshot = ActivitySnapshot::from(activity);
            write_activity(paths, &snapshot)?;
            Ok(Some(snapshot))
        }
        HookAction::Touch {
            provider,
            activity_id,
            generation_id,
            workspace_roots,
            event,
        } => touch_activity(
            paths,
            provider,
            &activity_id,
            generation_id,
            workspace_roots,
            None,
            event,
        ),
        HookAction::SetExisting {
            provider,
            activity_id,
            generation_id,
            workspace_roots,
            state,
            event,
        } => touch_activity(
            paths,
            provider,
            &activity_id,
            generation_id,
            workspace_roots,
            Some(state),
            event,
        ),
        HookAction::RemoveConversation {
            provider,
            conversation_id,
            ..
        } => {
            remove_conversation(paths, provider, &conversation_id)?;
            Ok(None)
        }
        HookAction::Ignore { .. } => Ok(None),
    }
}

pub fn write_activity(paths: &Paths, snapshot: &ActivitySnapshot) -> Result<()> {
    let path = activity_path(paths, snapshot.provider, &snapshot.activity_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut merged = snapshot.clone();
    if let Some(existing) = read_activity(&path)? {
        if merged.conversation_id.is_none() {
            merged.conversation_id = existing.conversation_id;
        }
        if merged.generation_id.is_none() {
            merged.generation_id = existing.generation_id;
        }
        if merged.workspace_roots.is_empty() {
            merged.workspace_roots = existing.workspace_roots;
        }
    }
    atomic_write_json(&path, &merged)
}

fn touch_activity(
    paths: &Paths,
    provider: Provider,
    activity_id: &str,
    generation_id: Option<String>,
    workspace_roots: Vec<String>,
    state: Option<LightState>,
    event: String,
) -> Result<Option<ActivitySnapshot>> {
    let path = activity_path(paths, provider, activity_id);
    let Some(mut snapshot) = read_activity(&path)? else {
        return Ok(None);
    };
    snapshot.updated_at = Utc::now();
    snapshot.event = event;
    if let Some(state) = state {
        snapshot.state = state;
    }
    if generation_id.is_some() {
        snapshot.generation_id = generation_id;
    }
    if !workspace_roots.is_empty() {
        snapshot.workspace_roots = workspace_roots;
    }
    atomic_write_json(&path, &snapshot)?;
    Ok(Some(snapshot))
}

pub fn read_activities(paths: &Paths) -> Result<Vec<ActivitySnapshot>> {
    let mut activities = Vec::new();
    for provider in Provider::ALL {
        let dir = paths.provider_activities_dir(provider);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Some(snapshot) = read_activity(&path)? {
                // Do not allow a misplaced or manually copied file to cross
                // provider cleanup boundaries.
                if snapshot.provider == provider {
                    activities.push(snapshot);
                }
            }
        }
    }
    Ok(activities)
}

fn read_activity(path: &Path) -> Result<Option<ActivitySnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let snapshot =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(snapshot))
}

pub fn remove_conversation(
    paths: &Paths,
    provider: Provider,
    conversation_id: &str,
) -> Result<usize> {
    let dir = paths.provider_activities_dir(provider);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(snapshot) = read_activity(&path)? else {
            continue;
        };
        if snapshot.conversation_id.as_deref() == Some(conversation_id)
            || snapshot.activity_id == conversation_id
        {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn remove_provider_activities(paths: &Paths, provider: Provider) -> Result<usize> {
    let dir = paths.provider_activities_dir(provider);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn clear_all_activities(paths: &Paths) -> Result<usize> {
    let mut removed = 0;
    for provider in Provider::ALL {
        removed += remove_provider_activities(paths, provider)?;
    }
    Ok(removed)
}

/// Resolve all provider activities plus the legacy Codex snapshot and rollout
/// fallback into the lamps that should be displayed together.
///
/// Hook activities are authoritative for their conversation. This prevents a
/// Codex rollout's older `working` fallback from manufacturing a yellow lamp
/// for the same conversation while its hook reports `waiting`.
pub fn resolve_display_state(
    legacy: Option<&StatusSnapshot>,
    activities: &[ActivitySnapshot],
    codex_sessions: &[SessionSnapshot],
    resets: &ProviderResets,
) -> DisplayState {
    let now = Utc::now();
    let mut states = BTreeMap::<String, (u8, LightState)>::new();

    for activity in activities {
        insert_source(
            &mut states,
            format!("{}:{}", activity.provider, activity.activity_id),
            3,
            effective_state(
                activity.state,
                activity.updated_at,
                resets.get(&activity.provider).copied(),
                now,
            ),
        );
    }

    let codex_reset = resets.get(&Provider::Codex).copied();
    for session in codex_sessions {
        let identity = session
            .session_id
            .as_deref()
            .map(|id| format!("codex:{id}"))
            .unwrap_or_else(|| format!("codex-rollout:{}", session.path.display()));
        insert_source(
            &mut states,
            identity,
            1,
            effective_state(session.state, session.updated_at, codex_reset, now),
        );
    }

    if let Some(legacy) = legacy.filter(|snapshot| snapshot.age() < chrono::Duration::hours(2)) {
        let identity = legacy
            .session_id
            .as_deref()
            .map(|id| format!("codex:{id}"))
            .unwrap_or_else(|| "codex-legacy".into());
        insert_source(
            &mut states,
            identity,
            2,
            effective_state(legacy.state, legacy.updated_at, codex_reset, now),
        );
    }

    DisplayState::from_states(states.into_values().map(|(_, state)| state))
}

fn insert_source(
    states: &mut BTreeMap<String, (u8, LightState)>,
    identity: String,
    precedence: u8,
    state: LightState,
) {
    // Idle sources carry no display information and must not mask a fresher
    // lower-precedence fallback.
    if state == LightState::Idle {
        return;
    }
    match states.get_mut(&identity) {
        Some((current_precedence, current_state)) if *current_precedence > precedence => {}
        Some((current_precedence, current_state)) if *current_precedence == precedence => {
            if state.priority() > current_state.priority() {
                *current_state = state;
            }
        }
        Some(current) => *current = (precedence, state),
        None => {
            states.insert(identity, (precedence, state));
        }
    }
}

fn effective_state(
    state: LightState,
    updated_at: DateTime<Utc>,
    reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> LightState {
    let active = matches!(state, LightState::Working | LightState::Waiting);
    if active && reset_at.is_some_and(|reset| updated_at <= reset) {
        return LightState::Idle;
    }

    let stale_after = chrono::Duration::from_std(WORKING_IDLE_AFTER).unwrap();
    if active && now - updated_at >= stale_after {
        return LightState::Idle;
    }

    apply_done_timeout(state, Some(updated_at))
}

fn activity_path(paths: &Paths, provider: Provider, activity_id: &str) -> PathBuf {
    let hash = fnv1a64(activity_id.as_bytes());
    paths
        .provider_activities_dir(provider)
        .join(format!("{hash:016x}.json"))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let unique = format!(
        "tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let tmp = path.with_extension(unique);
    let body = serde_json::to_vec_pretty(value)?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn paths_in_tmp() -> (tempfile::TempDir, Paths) {
        let dir = tempdir().unwrap();
        let paths = Paths {
            home: dir.path().to_path_buf(),
            codex_home: None,
        };
        (dir, paths)
    }

    fn activity(provider: Provider, id: &str, state: LightState) -> ActivitySnapshot {
        ActivitySnapshot {
            provider,
            activity_id: id.into(),
            conversation_id: Some(id.into()),
            generation_id: None,
            workspace_roots: vec![format!("/{id}")],
            state,
            updated_at: Utc::now(),
            source: "hook".into(),
            event: "test".into(),
        }
    }

    #[test]
    fn stores_each_provider_and_conversation_separately() {
        let (_dir, paths) = paths_in_tmp();
        write_activity(
            &paths,
            &activity(Provider::Codex, "codex-a", LightState::Working),
        )
        .unwrap();
        write_activity(
            &paths,
            &activity(Provider::Cursor, "cursor-a", LightState::Done),
        )
        .unwrap();
        write_activity(
            &paths,
            &activity(Provider::Cursor, "cursor-b", LightState::Waiting),
        )
        .unwrap();

        let loaded = read_activities(&paths).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            resolve_display_state(None, &loaded, &[], &ProviderResets::new()),
            DisplayState::WAITING_AND_WORKING
        );
    }

    #[test]
    fn completing_one_conversation_does_not_hide_another_worker() {
        let activities = [
            activity(Provider::Codex, "codex-a", LightState::Working),
            activity(Provider::Cursor, "cursor-a", LightState::Done),
        ];
        assert_eq!(
            resolve_display_state(None, &activities, &[], &ProviderResets::new()),
            DisplayState::WORKING
        );
    }

    #[test]
    fn terminal_event_preserves_existing_workspace_metadata() {
        let (_dir, paths) = paths_in_tmp();
        let initial = activity(Provider::Cursor, "cursor-a", LightState::Working);
        write_activity(&paths, &initial).unwrap();

        let mut done = activity(Provider::Cursor, "cursor-a", LightState::Done);
        done.workspace_roots.clear();
        done.generation_id = None;
        write_activity(&paths, &done).unwrap();

        let loaded = read_activities(&paths).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].workspace_roots, vec!["/cursor-a"]);
        assert_eq!(loaded[0].state, LightState::Done);
    }

    #[test]
    fn provider_reset_only_clears_that_provider() {
        let mut codex = activity(Provider::Codex, "codex-a", LightState::Working);
        let mut cursor = activity(Provider::Cursor, "cursor-a", LightState::Working);
        let reset = Utc::now();
        codex.updated_at = reset - chrono::Duration::seconds(1);
        cursor.updated_at = reset + chrono::Duration::seconds(1);
        let mut resets = ProviderResets::new();
        resets.insert(Provider::Codex, reset);

        assert_eq!(
            resolve_display_state(None, &[codex, cursor], &[], &resets),
            DisplayState::WORKING
        );
    }

    #[test]
    fn waiting_and_working_conversations_light_both_lamps() {
        let activities = [
            activity(Provider::Codex, "codex-waiting", LightState::Waiting),
            activity(Provider::Cursor, "cursor-working", LightState::Working),
        ];
        assert_eq!(
            resolve_display_state(None, &activities, &[], &ProviderResets::new()),
            DisplayState::WAITING_AND_WORKING
        );
    }

    #[test]
    fn same_codex_conversation_does_not_manufacture_yellow_fallback() {
        let waiting = activity(Provider::Codex, "same", LightState::Waiting);
        let sessions = [SessionSnapshot {
            path: PathBuf::from("rollout-same.jsonl"),
            session_id: Some("same".into()),
            state: LightState::Working,
            updated_at: Utc::now(),
        }];
        assert_eq!(
            resolve_display_state(None, &[waiting], &sessions, &ProviderResets::new()),
            DisplayState::WAITING
        );
    }

    #[test]
    fn active_conversations_hide_completed_lamp() {
        let activities = [
            activity(Provider::Codex, "done", LightState::Done),
            activity(Provider::Cursor, "working", LightState::Working),
        ];
        assert_eq!(
            resolve_display_state(None, &activities, &[], &ProviderResets::new()),
            DisplayState::WORKING
        );
    }

    #[test]
    fn remove_conversation_is_scoped_to_provider() {
        let (_dir, paths) = paths_in_tmp();
        write_activity(
            &paths,
            &activity(Provider::Codex, "same", LightState::Working),
        )
        .unwrap();
        write_activity(
            &paths,
            &activity(Provider::Cursor, "same", LightState::Working),
        )
        .unwrap();

        assert_eq!(
            remove_conversation(&paths, Provider::Cursor, "same").unwrap(),
            1
        );
        let loaded = read_activities(&paths).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].provider, Provider::Codex);
    }

    #[test]
    fn touch_does_not_manufacture_activity() {
        let (_dir, paths) = paths_in_tmp();
        let touched = touch_activity(
            &paths,
            Provider::Cursor,
            "missing",
            None,
            vec![],
            None,
            "preToolUse".into(),
        )
        .unwrap();
        assert!(touched.is_none());
        assert!(read_activities(&paths).unwrap().is_empty());
    }
}
