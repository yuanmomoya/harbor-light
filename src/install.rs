use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::providers::Provider;

#[cfg(target_os = "macos")]
use crate::paths::{append_log, Paths};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{install, package, uninstall};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn install(
    _paths: &crate::paths::Paths,
    _dest: Option<PathBuf>,
    _skip_launch: bool,
    _skip_bundle: bool,
) -> Result<PathBuf> {
    bail!("Harbor Light 安装目前支持 macOS 和 Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn package(_out: PathBuf, _zip: bool) -> Result<PathBuf> {
    bail!("Harbor Light 打包目前支持 macOS 和 Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn uninstall(_paths: &crate::paths::Paths, _dest: Option<PathBuf>) -> Result<()> {
    bail!("Harbor Light 卸载目前支持 macOS 和 Windows")
}

#[cfg(target_os = "macos")]
pub const BUNDLE_ID: &str = "com.harborlight.app";
#[cfg(target_os = "macos")]
const LEGACY_BUNDLE_ID: &str = "com.codexlight.app";
#[cfg(target_os = "macos")]
pub const APP_NAME: &str = "HarborLight.app";
#[cfg(target_os = "macos")]
pub const BIN_NAME: &str = "harbor-light";
#[cfg(target_os = "macos")]
const APP_ICON_ICNS: &[u8] = include_bytes!("../resources/AppIcon.icns");
#[cfg(target_os = "macos")]
pub fn default_app_destination() -> PathBuf {
    let user_apps = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join("Applications");
    let system = PathBuf::from("/Applications").join(APP_NAME);
    if system.exists() {
        return system;
    }
    if user_apps.join(APP_NAME).exists() {
        return user_apps.join(APP_NAME);
    }
    if can_write_dir(Path::new("/Applications")) {
        system
    } else {
        let _ = fs::create_dir_all(&user_apps);
        user_apps.join(APP_NAME)
    }
}

#[cfg(target_os = "macos")]
fn can_write_dir(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let probe = path.join(".harborlight-write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn current_executable() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("current_exe")?;
    fs::canonicalize(&exe).or(Ok(exe))
}

pub fn hook_command_for(binary: &Path, provider: Provider) -> String {
    let mut display = binary.display().to_string();
    if let Some(stripped) = display
        .strip_prefix(r"\\?\")
        .or_else(|| display.strip_prefix(r"//?/"))
    {
        display = stripped.to_string();
    }
    if display.contains(' ') {
        format!("\"{display}\" hook --provider {}", provider.as_str())
    } else {
        format!("{display} hook --provider {}", provider.as_str())
    }
}

const CURSOR_HOOK_TIMEOUT_SECS: u64 = 8;

pub fn is_our_hook_command(command: &str) -> bool {
    let normalized = command.replace('"', "").trim().to_ascii_lowercase();
    let has_hook = normalized.split_whitespace().any(|part| part == "hook");
    if !has_hook {
        return false;
    }
    normalized.contains("harbor-light")
        || normalized.contains("harborlight")
        || normalized.contains("codex-light")
        || normalized.contains("codexlight")
}

pub fn merge_codex_hooks(hooks_file: &Path, command: &str) -> Result<()> {
    if let Some(parent) = hooks_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut root = if hooks_file.exists() {
        let raw = fs::read_to_string(hooks_file)?;
        if raw.trim().is_empty() {
            json!({ "hooks": {} })
        } else {
            serde_json::from_str(&raw).with_context(|| format!("parse {}", hooks_file.display()))?
        }
    } else {
        json!({ "hooks": {} })
    };

    if !root.is_object() {
        bail!("hooks.json root must be an object");
    }
    if root.get("hooks").is_none() {
        root["hooks"] = json!({});
    }
    let hooks = root
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .context("hooks.json missing hooks object")?;

    for event in Provider::Codex.hook_events() {
        let entry = hook_entry(command);
        let list = hooks
            .entry((*event).to_string())
            .or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let arr = list.as_array_mut().unwrap();
        if group_has_our_hook(arr) {
            replace_our_command(arr, command);
        } else {
            arr.push(entry);
        }
    }

    atomic_write(hooks_file, &serde_json::to_vec_pretty(&root)?)?;
    Ok(())
}

/// Merge native Cursor hooks. Cursor uses a flat list of command definitions,
/// unlike Codex's nested hook groups.
pub fn merge_cursor_hooks(hooks_file: &Path, command: &str) -> Result<()> {
    if let Some(parent) = hooks_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut root = if hooks_file.exists() {
        let raw = fs::read_to_string(hooks_file)?;
        if raw.trim().is_empty() {
            json!({ "version": 1, "hooks": {} })
        } else {
            serde_json::from_str(&raw).with_context(|| format!("parse {}", hooks_file.display()))?
        }
    } else {
        json!({ "version": 1, "hooks": {} })
    };

    if !root.is_object() {
        bail!("Cursor hooks.json root must be an object");
    }
    if root.get("version").is_none() {
        root["version"] = json!(1);
    }
    if root.get("hooks").is_none() {
        root["hooks"] = json!({});
    }
    let hooks = root
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .context("Cursor hooks.json missing hooks object")?;

    for event in Provider::Cursor.hook_events() {
        let list = hooks
            .entry((*event).to_string())
            .or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let entries = list.as_array_mut().unwrap();
        upsert_our_cursor_hook(entries, command);
    }

    atomic_write(hooks_file, &serde_json::to_vec_pretty(&root)?)?;
    Ok(())
}

fn upsert_our_cursor_hook(entries: &mut Vec<Value>, command: &str) {
    let mut replaced = false;
    entries.retain_mut(|entry| {
        let ours = entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(is_our_hook_command);
        if !ours {
            return true;
        }
        if replaced {
            return false;
        }
        entry["command"] = json!(command);
        entry["timeout"] = json!(CURSOR_HOOK_TIMEOUT_SECS);
        replaced = true;
        true
    });
    if !replaced {
        entries.push(json!({
            "command": command,
            "timeout": CURSOR_HOOK_TIMEOUT_SECS
        }));
    }
}

pub fn install_provider_hooks(paths: &crate::paths::Paths, binary: &Path) -> Result<()> {
    fs::create_dir_all(paths.activities_dir())?;
    let codex_command = hook_command_for(binary, Provider::Codex);
    merge_codex_hooks(&paths.hooks_file(), &codex_command)?;
    let cursor_command = hook_command_for(binary, Provider::Cursor);
    merge_cursor_hooks(&paths.cursor_hooks_file(), &cursor_command)?;
    Ok(())
}

fn hook_entry(command: &str) -> Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 3,
            "statusMessage": "Updating Harbor Light"
        }]
    })
}

fn group_has_our_hook(groups: &[Value]) -> bool {
    groups.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(is_our_hook_command)
                })
            })
            .unwrap_or(false)
    })
}

fn replace_our_command(groups: &mut [Value], command: &str) {
    for group in groups {
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for hook in hooks {
                if hook
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_our_hook_command)
                {
                    hook["command"] = json!(command);
                    hook["timeout"] = json!(3);
                }
            }
        }
    }
}

pub fn remove_our_codex_hooks(hooks_file: &Path) -> Result<bool> {
    if !hooks_file.exists() {
        return Ok(false);
    }
    let raw = fs::read_to_string(hooks_file)?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let mut root: Value = serde_json::from_str(&raw)?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let mut changed = false;
    for event in Provider::Codex.hook_events() {
        let Some(list) = hooks.get_mut(*event).and_then(Value::as_array_mut) else {
            continue;
        };
        for group in list.iter_mut() {
            if let Some(inner) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let inner_before = inner.len();
                inner.retain(|h| {
                    !h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(is_our_hook_command)
                });
                if inner.len() != inner_before {
                    changed = true;
                }
            }
        }
        let before = list.len();
        list.retain(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map(|h| !h.is_empty())
                .unwrap_or(true)
        });
        if list.len() != before {
            changed = true;
        }
        if list.is_empty() {
            hooks.remove(*event);
            changed = true;
        }
    }
    if changed {
        atomic_write(hooks_file, &serde_json::to_vec_pretty(&root)?)?;
    }
    Ok(changed)
}

pub fn remove_our_cursor_hooks(hooks_file: &Path) -> Result<bool> {
    if !hooks_file.exists() {
        return Ok(false);
    }
    let raw = fs::read_to_string(hooks_file)?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let mut root: Value = serde_json::from_str(&raw)?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let mut changed = false;
    for event in Provider::Cursor.hook_events() {
        let Some(list) = hooks.get_mut(*event).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = list.len();
        list.retain(|entry| {
            !entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_our_hook_command)
        });
        if list.len() != before {
            changed = true;
        }
        if list.is_empty() {
            hooks.remove(*event);
        }
    }
    if changed {
        atomic_write(hooks_file, &serde_json::to_vec_pretty(&root)?)?;
    }
    Ok(changed)
}

pub fn remove_all_provider_hooks(paths: &crate::paths::Paths) -> Result<()> {
    let _ = remove_our_codex_hooks(&paths.hooks_file())?;
    let _ = remove_our_cursor_hooks(&paths.cursor_hooks_file())?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn write_launch_agent(plist: &Path, app_binary: &Path) -> Result<()> {
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{BUNDLE_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        app_binary.display()
    );
    fs::write(plist, body)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>{BIN_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>{BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>HarborLight</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIconName</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

#[cfg(target_os = "macos")]
pub fn assemble_app_bundle(dest: &Path, binary: &Path) -> Result<PathBuf> {
    let macos = dest.join("Contents/MacOS");
    let resources = dest.join("Contents/Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::write(dest.join("Contents/Info.plist"), info_plist())?;
    fs::write(dest.join("Contents/PkgInfo"), b"APPL????")?;
    fs::write(resources.join("AppIcon.icns"), APP_ICON_ICNS)
        .with_context(|| format!("write icon into {}", resources.display()))?;
    let dest_bin = macos.join(BIN_NAME);
    fs::copy(binary, &dest_bin)
        .with_context(|| format!("copy {} -> {}", binary.display(), dest_bin.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest_bin)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest_bin, perms)?;
    }
    let _ = Command::new("codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(dest)
        .status();
    Ok(dest_bin)
}

#[cfg(target_os = "macos")]
pub fn launchctl_bootout() {
    if let Ok(uid) = user_id() {
        for label in [BUNDLE_ID, LEGACY_BUNDLE_ID] {
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{label}")])
                .status();
        }
        let paths = Paths::current();
        for plist in [
            paths.launch_agent_plist(),
            paths.launch_agents_dir().join("com.codexlight.app.plist"),
        ] {
            let _ = Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(plist.to_string_lossy().as_ref())
                .status();
        }
    }
}

#[cfg(target_os = "macos")]
pub fn launchctl_bootstrap(plist: &Path) -> Result<()> {
    let uid = user_id()?;
    let domain = format!("gui/{uid}");
    for label in [BUNDLE_ID, LEGACY_BUNDLE_ID] {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{label}")])
            .status();
    }
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(plist)
        .status()
        .context("launchctl bootstrap")?;
    if !status.success() {
        // Older macOS fallback.
        let fallback = Command::new("launchctl")
            .args(["load", "-w"])
            .arg(plist)
            .status()
            .context("launchctl load")?;
        if !fallback.success() {
            bail!("failed to register LaunchAgent");
        }
    }
    let _ = Command::new("launchctl")
        .args(["enable", &format!("{domain}/{BUNDLE_ID}")])
        .status();
    let _ = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("{domain}/{BUNDLE_ID}")])
        .status();
    Ok(())
}

#[cfg(target_os = "macos")]
fn user_id() -> Result<u32> {
    let output = Command::new("id").arg("-u").output().context("id -u")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().parse().context("parse uid")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install(
    paths: &Paths,
    dest: Option<PathBuf>,
    skip_launch: bool,
    skip_bundle: bool,
) -> Result<PathBuf> {
    let dest = dest.unwrap_or_else(default_app_destination);
    let app_bin = if skip_bundle {
        let bin = dest.join("Contents/MacOS").join(BIN_NAME);
        if !bin.is_file() {
            bail!("找不到已安装的 App：{}", bin.display());
        }
        bin
    } else {
        let src = current_executable()?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if dest.exists() {
            let _ = fs::remove_dir_all(&dest);
        }
        assemble_app_bundle(&dest, &src)?
    };
    install_provider_hooks(paths, &app_bin)?;
    write_launch_agent(&paths.launch_agent_plist(), &app_bin)?;
    let legacy_plist = paths.launch_agents_dir().join("com.codexlight.app.plist");
    if legacy_plist.exists() {
        let _ = fs::remove_file(&legacy_plist);
    }
    if !skip_launch {
        launchctl_bootstrap(&paths.launch_agent_plist())?;
    }
    append_log(
        paths,
        &format!(
            "installed app={} providers=codex,cursor launchagent={} skip_bundle={skip_bundle}",
            dest.display(),
            paths.launch_agent_plist().display()
        ),
    );
    println!("已安装 {}", dest.display());
    println!("已配置 Codex 和 Cursor 用户级 Hooks。");
    println!("首次在 Codex 里触发 Hook 时，请在弹窗或 /hooks 中允许信任。");
    Ok(dest)
}

/// Build a distributable `.app` (no hooks / LaunchAgent). Optionally zip it.
#[cfg(target_os = "macos")]
pub fn package(out: PathBuf, zip: bool) -> Result<PathBuf> {
    let src = current_executable()?;
    let dest = if out.extension().is_some_and(|ext| ext == "app") {
        out
    } else {
        fs::create_dir_all(&out)?;
        out.join(APP_NAME)
    };
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    assemble_app_bundle(&dest, &src)?;
    let dest = dest
        .canonicalize()
        .with_context(|| "canonicalize app bundle")?;
    println!("已打包 {}", dest.display());

    if zip {
        let zip_path = dest.with_extension("zip");
        if zip_path.exists() {
            fs::remove_file(&zip_path)?;
        }
        let status = Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&dest)
            .arg(&zip_path)
            .status()
            .context("ditto zip")?;
        if !status.success() {
            bail!("zip 失败: {}", zip_path.display());
        }
        println!("已压缩 {}", zip_path.display());
    }
    Ok(dest)
}

#[cfg(target_os = "macos")]
pub fn uninstall(paths: &Paths, dest: Option<PathBuf>) -> Result<()> {
    launchctl_bootout();
    let plist = paths.launch_agent_plist();
    if plist.exists() {
        fs::remove_file(&plist).ok();
    }
    let legacy_plist = paths.launch_agents_dir().join("com.codexlight.app.plist");
    if legacy_plist.exists() {
        fs::remove_file(&legacy_plist).ok();
    }
    let _ = remove_all_provider_hooks(paths);
    let dest = dest.unwrap_or_else(default_app_destination);
    let remove_app = dest.exists().then(|| fs::remove_dir_all(&dest));
    let _ = fs::remove_file(paths.status_file());
    let _ = fs::remove_file(paths.log_file());
    let _ = fs::remove_file(paths.home.join(".codex-status-light.log"));
    let _ = crate::activity::clear_all_activities(paths);
    let _ = fs::remove_dir_all(paths.light_dir());
    let _ = fs::remove_dir_all(paths.home.join(".codex-light"));
    if let Some(result) = remove_app {
        result.with_context(|| format!("无法删除 {}，请使用管理员权限重试", dest.display()))?;
    }
    println!("已卸载 Harbor Light。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn merge_preserves_existing_hooks() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hooks.json");
        fs::write(
            &file,
            r#"{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "/usr/bin/true" } ] }
    ]
  }
}"#,
        )
        .unwrap();
        merge_codex_hooks(&file, "/tmp/harbor-light hook --provider codex").unwrap();
        let raw = fs::read_to_string(&file).unwrap();
        assert!(raw.contains("/usr/bin/true"));
        assert!(raw.contains("/tmp/harbor-light hook"));
        assert!(raw.contains("UserPromptSubmit"));
        assert!(raw.contains("PermissionRequest"));
        assert!(raw.contains("PostToolUse"));
        // second merge is idempotent
        merge_codex_hooks(
            &file,
            "/opt/HarborLight.app/Contents/MacOS/harbor-light hook --provider codex",
        )
        .unwrap();
        let raw2 = fs::read_to_string(&file).unwrap();
        assert_eq!(
            raw2.matches("harbor-light hook").count(),
            Provider::Codex.hook_events().len()
        );
        assert!(!raw2.contains("/tmp/harbor-light hook"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn launch_agent_only_restarts_after_unsuccessful_exit() {
        let dir = tempdir().unwrap();
        let plist = dir.path().join("com.harborlight.app.plist");
        write_launch_agent(
            &plist,
            Path::new("/Applications/HarborLight.app/Contents/MacOS/harbor-light"),
        )
        .unwrap();

        let raw = fs::read_to_string(plist).unwrap();
        assert!(raw.contains("<key>SuccessfulExit</key>\n        <false/>"));
        assert!(!raw.contains("<key>KeepAlive</key>\n    <true/>"));
    }

    #[test]
    fn uninstall_only_removes_our_entries() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hooks.json");
        merge_codex_hooks(&file, "/tmp/harbor-light hook --provider codex").unwrap();
        let mut root: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        root["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [ { "type": "command", "command": "echo keep" } ] }));
        fs::write(&file, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
        assert!(remove_our_codex_hooks(&file).unwrap());
        let raw = fs::read_to_string(&file).unwrap();
        assert!(raw.contains("echo keep"));
        assert!(!raw.contains("harbor-light hook"));
        assert!(!raw.contains("SessionStart"));
    }

    #[test]
    fn detects_our_command() {
        assert!(is_our_hook_command(
            "/Applications/HarborLight.app/Contents/MacOS/harbor-light hook"
        ));
        assert!(is_our_hook_command(
            "/Applications/HarborLight.app/Contents/MacOS/harbor-light hook --provider cursor"
        ));
        assert!(is_our_hook_command(
            "\"/Users/me/Applications/HarborLight.app/Contents/MacOS/harbor-light\" hook"
        ));
        assert!(is_our_hook_command(
            r#""C:\Users\me\AppData\Local\HarborLight\HarborLight.exe" hook"#
        ));
        assert!(is_our_hook_command(
            r#"C:\Users\me\AppData\Local\HarborLight\HarborLight-bom-fix.exe hook --provider cursor"#
        ));
        assert!(is_our_hook_command(
            "/Applications/CodexLight.app/Contents/MacOS/codex-light hook"
        ));
        assert!(is_our_hook_command(
            r#""C:\Users\me\AppData\Local\CodexLight\CodexLight.exe" hook"#
        ));
        assert!(!is_our_hook_command("python3 ~/.codex/hooks/session.py"));
    }

    #[test]
    fn hook_command_strips_windows_verbatim_prefix() {
        let command = hook_command_for(
            Path::new(r"\\?\C:\Users\me\AppData\Local\HarborLight\HarborLight.exe"),
            Provider::Cursor,
        );
        assert_eq!(
            command,
            r"C:\Users\me\AppData\Local\HarborLight\HarborLight.exe hook --provider cursor"
        );
        assert!(!command.contains(r"\\?\"));
    }

    #[test]
    fn upgrades_legacy_codex_light_hook_commands() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hooks.json");
        merge_codex_hooks(&file, "/tmp/codex-light hook --provider codex").unwrap();
        merge_codex_hooks(
            &file,
            "/opt/HarborLight.app/Contents/MacOS/harbor-light hook --provider codex",
        )
        .unwrap();
        let raw = fs::read_to_string(&file).unwrap();
        assert_eq!(
            raw.matches("harbor-light hook").count(),
            Provider::Codex.hook_events().len()
        );
        assert!(!raw.contains("codex-light hook"));
    }

    #[test]
    fn cursor_merge_is_flat_idempotent_and_preserves_existing_hooks() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hooks.json");
        fs::write(
            &file,
            r#"{"version":1,"hooks":{"stop":[{"command":"echo keep"}]}}"#,
        )
        .unwrap();

        merge_cursor_hooks(
            &file,
            "/Applications/HarborLight.app/Contents/MacOS/harbor-light hook --provider cursor",
        )
        .unwrap();
        merge_cursor_hooks(
            &file,
            "/opt/HarborLight.app/Contents/MacOS/harbor-light hook --provider cursor",
        )
        .unwrap();

        let raw = fs::read_to_string(&file).unwrap();
        let root: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(root["version"], json!(1));
        assert!(root["hooks"]["beforeShellExecution"].is_array());
        assert!(root["hooks"]["afterShellExecution"].is_array());
        assert!(root["hooks"]["beforeMCPExecution"].is_array());
        assert!(root["hooks"]["afterMCPExecution"].is_array());
        assert_eq!(
            raw.matches("harbor-light hook --provider cursor").count(),
            Provider::Cursor.hook_events().len()
        );
        assert!(raw.contains("echo keep"));
        assert!(!raw.contains("/Applications/HarborLight.app"));

        assert!(remove_our_cursor_hooks(&file).unwrap());
        let raw = fs::read_to_string(&file).unwrap();
        assert!(raw.contains("echo keep"));
        assert!(!raw.contains("harbor-light hook"));
    }

    #[test]
    fn cursor_merge_collapses_duplicate_harbor_light_binaries() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hooks.json");
        fs::write(
            &file,
            r#"{
              "version": 1,
              "hooks": {
                "stop": [
                  {"command": "C:\\Users\\me\\AppData\\Local\\HarborLight\\HarborLight-bom-fix.exe hook --provider cursor", "timeout": 3},
                  {"command": "C:\\Users\\me\\AppData\\Local\\HarborLight\\HarborLight.exe hook --provider cursor", "timeout": 3},
                  {"command": "echo keep"}
                ]
              }
            }"#,
        )
        .unwrap();

        merge_cursor_hooks(
            &file,
            r#""C:\Users\me\AppData\Local\HarborLight\HarborLight.exe" hook --provider cursor"#,
        )
        .unwrap();

        let root: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        let stop = root["hooks"]["stop"].as_array().unwrap();
        let ours: Vec<_> = stop
            .iter()
            .filter(|entry| {
                entry
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_our_hook_command)
            })
            .collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0]["timeout"], json!(8));
        assert!(ours[0]["command"]
            .as_str()
            .unwrap()
            .contains("HarborLight.exe"));
        assert!(!serde_json::to_string(&root)
            .unwrap()
            .contains("HarborLight-bom-fix.exe"));
        assert!(stop.iter().any(|entry| {
            entry.get("command").and_then(Value::as_str) == Some("echo keep")
        }));
    }
}
