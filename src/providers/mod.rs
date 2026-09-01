mod codex;
mod cursor;

use std::fmt;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::status::LightState;

/// A supported coding-agent application.
///
/// Provider-specific payload parsing and platform metadata live under this
/// module. The rest of the application only consumes normalized activities.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Cursor,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::Codex, Self::Cursor];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    pub const fn hook_events(self) -> &'static [&'static str] {
        match self {
            Self::Codex => codex::HOOK_EVENTS,
            Self::Cursor => cursor::HOOK_EVENTS,
        }
    }

    #[cfg(target_os = "macos")]
    pub const fn macos_bundle_ids(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["com.openai.codex"],
            Self::Cursor => &["com.todesktop.230313mzl4w4u92"],
        }
    }

    #[cfg(target_os = "windows")]
    pub const fn windows_process_names(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["chatgpt.exe", "codex.exe"],
            Self::Cursor => &["cursor.exe"],
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedActivity {
    pub provider: Provider,
    pub activity_id: String,
    pub conversation_id: Option<String>,
    pub generation_id: Option<String>,
    pub workspace_roots: Vec<String>,
    pub state: LightState,
    pub event: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    Upsert(NormalizedActivity),
    Touch {
        provider: Provider,
        activity_id: String,
        generation_id: Option<String>,
        workspace_roots: Vec<String>,
        event: String,
    },
    /// Update an existing activity's state. Never creates a new conversation,
    /// so a late tool-completion event cannot revive a finished turn.
    SetExisting {
        provider: Provider,
        activity_id: String,
        generation_id: Option<String>,
        workspace_roots: Vec<String>,
        state: LightState,
        event: String,
    },
    RemoveConversation {
        provider: Provider,
        conversation_id: String,
        event: String,
    },
    Ignore {
        event: Option<String>,
        reason: &'static str,
    },
}

/// Provider adapters turn vendor-specific hook JSON into common operations.
pub trait HookProvider: Sync {
    fn provider(&self) -> Provider;
    fn normalize(&self, payload: Value) -> Result<HookAction>;
}

pub fn adapter(provider: Provider) -> &'static dyn HookProvider {
    match provider {
        Provider::Codex => &codex::CODEX,
        Provider::Cursor => &cursor::CURSOR,
    }
}

pub(crate) fn activity_id(
    conversation_id: Option<&str>,
    session_id: Option<&str>,
    generation_id: Option<&str>,
    workspace: Option<&str>,
) -> Option<String> {
    conversation_id
        .or(session_id)
        .or(generation_id)
        .or(workspace)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}
