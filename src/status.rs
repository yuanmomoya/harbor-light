use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

pub const DONE_IDLE_AFTER: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum LightState {
    Idle = 0,
    Done = 1,
    Working = 2,
    Waiting = 3,
}

/// The lamps that should be visible at the same time.
///
/// Conversation state remains a single `LightState`, but the UI may need to
/// show both waiting and working when different conversations are in those
/// states. Green is intentionally exclusive: completed work is only surfaced
/// when there are no active conversations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayState(u8);

impl DisplayState {
    const RED: u8 = 1 << 0;
    const YELLOW: u8 = 1 << 1;
    const GREEN: u8 = 1 << 2;

    pub const IDLE: Self = Self(0);
    pub const DONE: Self = Self(Self::GREEN);
    #[cfg(any(test, target_os = "macos"))]
    pub const WORKING: Self = Self(Self::YELLOW);
    #[cfg(any(test, target_os = "macos"))]
    pub const WAITING: Self = Self(Self::RED);
    #[cfg(any(test, target_os = "macos"))]
    pub const WAITING_AND_WORKING: Self = Self(Self::RED | Self::YELLOW);

    pub fn from_states<I>(states: I) -> Self
    where
        I: IntoIterator<Item = LightState>,
    {
        let mut waiting = false;
        let mut working = false;
        let mut done = false;
        for state in states {
            waiting |= state == LightState::Waiting;
            working |= state == LightState::Working;
            done |= state == LightState::Done;
        }

        if waiting || working {
            Self(((waiting as u8) * Self::RED) | ((working as u8) * Self::YELLOW))
        } else if done {
            Self::DONE
        } else {
            Self::IDLE
        }
    }

    pub const fn red_active(self) -> bool {
        self.0 & Self::RED != 0
    }

    pub const fn yellow_active(self) -> bool {
        self.0 & Self::YELLOW != 0
    }

    pub const fn green_active(self) -> bool {
        self.0 & Self::GREEN != 0
    }

    pub const fn is_idle(self) -> bool {
        self.0 == 0
    }

    #[cfg(target_os = "macos")]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    #[cfg(target_os = "macos")]
    pub const fn from_u8(value: u8) -> Self {
        Self(value & (Self::RED | Self::YELLOW | Self::GREEN))
    }

    pub const fn label_zh(self) -> &'static str {
        match self.0 {
            Self::RED => "等待确认",
            Self::YELLOW => "工作中",
            Self::GREEN => "已完成",
            value if value == Self::RED | Self::YELLOW => "等待确认 + 工作中",
            _ => "空闲",
        }
    }
}

impl LightState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Done => "done",
        }
    }

    pub fn priority(self) -> u8 {
        self as u8
    }
}

impl std::fmt::Display for LightState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub state: LightState,
    pub updated_at: DateTime<Utc>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl StatusSnapshot {
    pub fn new(
        state: LightState,
        source: impl Into<String>,
        event: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            state,
            updated_at: Utc::now(),
            source: source.into(),
            event,
            session_id,
        }
    }

    pub fn age(&self) -> chrono::Duration {
        Utc::now() - self.updated_at
    }
}

pub fn read_status(path: &Path) -> Result<Option<StatusSnapshot>> {
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

/// Atomically replace the status file (`write tmp + rename`).
pub fn write_status(path: &Path, snapshot: &StatusSnapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(snapshot)?;
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
    Ok(())
}

pub fn write_current(paths: &Paths, snapshot: &StatusSnapshot) -> Result<()> {
    write_status(&paths.status_file(), snapshot)
}

pub fn read_current(paths: &Paths) -> Result<Option<StatusSnapshot>> {
    read_status(&paths.status_file())
}

#[cfg(test)]
pub fn aggregate_states<I>(states: I) -> LightState
where
    I: IntoIterator<Item = LightState>,
{
    states
        .into_iter()
        .max_by_key(|state| state.priority())
        .unwrap_or(LightState::Idle)
}

/// If the highest-priority state is `done` and it has been that way long
/// enough, collapse to `idle` so the light does not stay green forever.
pub fn apply_done_timeout(state: LightState, since: Option<DateTime<Utc>>) -> LightState {
    if state != LightState::Done {
        return state;
    }
    match since {
        Some(ts) if Utc::now() - ts >= chrono::Duration::from_std(DONE_IDLE_AFTER).unwrap() => {
            LightState::Idle
        }
        _ => LightState::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_atomic_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".codex-status.json");
        let snap = StatusSnapshot::new(
            LightState::Working,
            "hook",
            Some("UserPromptSubmit".into()),
            Some("abc".into()),
        );
        write_status(&path, &snap).unwrap();
        let loaded = read_status(&path).unwrap().unwrap();
        assert_eq!(loaded.state, LightState::Working);
        assert_eq!(loaded.source, "hook");
        assert_eq!(loaded.event.as_deref(), Some("UserPromptSubmit"));
        assert_eq!(loaded.session_id.as_deref(), Some("abc"));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn aggregate_priority() {
        assert_eq!(
            aggregate_states([LightState::Idle, LightState::Done, LightState::Working]),
            LightState::Working
        );
        assert_eq!(
            aggregate_states([LightState::Working, LightState::Waiting]),
            LightState::Waiting
        );
        assert_eq!(aggregate_states([]), LightState::Idle);
    }

    #[test]
    fn display_state_preserves_waiting_and_working_together() {
        let display = DisplayState::from_states([LightState::Waiting, LightState::Working]);
        assert_eq!(display, DisplayState::WAITING_AND_WORKING);
        assert!(display.red_active());
        assert!(display.yellow_active());
        assert!(!display.green_active());
    }

    #[test]
    fn active_lamps_suppress_completed_lamp() {
        assert_eq!(
            DisplayState::from_states([LightState::Done, LightState::Working]),
            DisplayState::WORKING
        );
        assert_eq!(
            DisplayState::from_states([LightState::Done, LightState::Waiting]),
            DisplayState::WAITING
        );
        assert_eq!(
            DisplayState::from_states([LightState::Done]),
            DisplayState::DONE
        );
    }

    #[test]
    fn done_collapses_after_timeout() {
        let old = Utc::now() - chrono::Duration::seconds(4);
        assert_eq!(
            apply_done_timeout(LightState::Done, Some(old)),
            LightState::Idle
        );
        assert_eq!(
            apply_done_timeout(LightState::Done, Some(Utc::now())),
            LightState::Done
        );
        assert_eq!(
            apply_done_timeout(LightState::Working, Some(old)),
            LightState::Working
        );
    }
}
