use std::path::PathBuf;

use crate::providers::Provider;

/// Filesystem locations used by Harbor Light.
///
/// Override the home directory with `HARBOR_LIGHT_HOME` (used in tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    pub codex_home: Option<PathBuf>,
}

impl Paths {
    pub fn current() -> Self {
        let light_home = std::env::var_os("HARBOR_LIGHT_HOME").map(PathBuf::from);
        let codex_home = light_home
            .is_none()
            .then(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
            .flatten();
        let home = light_home
            .or_else(dirs::home_dir)
            .expect("cannot determine home directory");
        Self { home, codex_home }
    }

    pub fn status_file(&self) -> PathBuf {
        self.home.join(".codex-status.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.home.join(".harbor-light.log")
    }

    pub fn light_dir(&self) -> PathBuf {
        self.home.join(".harbor-light")
    }

    pub fn activities_dir(&self) -> PathBuf {
        self.light_dir().join("activities")
    }

    pub fn provider_activities_dir(&self, provider: Provider) -> PathBuf {
        self.activities_dir().join(provider.as_str())
    }

    pub fn codex_dir(&self) -> PathBuf {
        self.codex_home
            .clone()
            .unwrap_or_else(|| self.home.join(".codex"))
    }

    #[cfg(target_os = "windows")]
    pub fn windows_position_file(&self) -> PathBuf {
        self.home.join(".harbor-light-window-windows.json")
    }

    pub fn hooks_file(&self) -> PathBuf {
        self.codex_dir().join("hooks.json")
    }

    pub fn cursor_hooks_file(&self) -> PathBuf {
        self.home.join(".cursor/hooks.json")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.codex_dir().join("sessions")
    }

    #[cfg(target_os = "macos")]
    pub fn launch_agents_dir(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    #[cfg(target_os = "macos")]
    pub fn launch_agent_plist(&self) -> PathBuf {
        self.launch_agents_dir().join("com.harborlight.app.plist")
    }
}

pub fn append_log(paths: &Paths, message: &str) {
    use std::io::Write;
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{ts} {message}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_standard_locations() {
        let paths = Paths {
            home: PathBuf::from("/Users/demo"),
            codex_home: None,
        };
        assert_eq!(
            paths.status_file(),
            PathBuf::from("/Users/demo/.codex-status.json")
        );
        assert_eq!(
            paths.hooks_file(),
            PathBuf::from("/Users/demo/.codex/hooks.json")
        );
        assert_eq!(
            paths.sessions_dir(),
            PathBuf::from("/Users/demo/.codex/sessions")
        );
        assert_eq!(
            paths.cursor_hooks_file(),
            PathBuf::from("/Users/demo/.cursor/hooks.json")
        );
        assert_eq!(
            paths.provider_activities_dir(Provider::Cursor),
            PathBuf::from("/Users/demo/.harbor-light/activities/cursor")
        );
        assert_eq!(
            paths.log_file(),
            PathBuf::from("/Users/demo/.harbor-light.log")
        );
    }

    #[test]
    fn respects_explicit_codex_home() {
        let paths = Paths {
            home: PathBuf::from("C:/Users/demo"),
            codex_home: Some(PathBuf::from("D:/shared-codex")),
        };
        assert_eq!(
            paths.hooks_file(),
            PathBuf::from("D:/shared-codex/hooks.json")
        );
        assert_eq!(
            paths.sessions_dir(),
            PathBuf::from("D:/shared-codex/sessions")
        );
    }
}
