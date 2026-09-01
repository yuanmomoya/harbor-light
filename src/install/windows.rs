use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

use crate::paths::{append_log, Paths};

use super::{current_executable, install_provider_hooks, remove_all_provider_hooks};

pub const APP_NAME: &str = "HarborLight.exe";
const WINDOW_CLASS_NAME: &str = "HarborLightTrafficWindow";
const LEGACY_WINDOW_CLASS_NAME: &str = "CodexLightTrafficWindow";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "HarborLight";
const LEGACY_RUN_VALUE: &str = "CodexLight";

pub fn default_app_destination() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join("AppData/Local")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("HarborLight").join(APP_NAME)
}

fn executable_destination(dest: Option<PathBuf>) -> PathBuf {
    let dest = dest.unwrap_or_else(default_app_destination);
    if dest
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        dest
    } else {
        dest.join(APP_NAME)
    }
}

fn set_autostart(binary: &Path) -> Result<()> {
    let value = autostart_value(binary);
    let status = Command::new("reg.exe")
        .args([
            "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &value, "/f",
        ])
        .status()
        .context("写入 Windows 开机自启注册表")?;
    if !status.success() {
        bail!("写入 Windows 开机自启注册表失败");
    }
    let _ = Command::new("reg.exe")
        .args(["delete", RUN_KEY, "/v", LEGACY_RUN_VALUE, "/f"])
        .status();
    Ok(())
}

fn autostart_value(binary: &Path) -> String {
    format!("\"{}\"", binary.display())
}

fn remove_autostart() {
    for value in [RUN_VALUE, LEGACY_RUN_VALUE] {
        let _ = Command::new("reg.exe")
            .args(["delete", RUN_KEY, "/v", value, "/f"])
            .status();
    }
}

pub fn install(
    paths: &Paths,
    dest: Option<PathBuf>,
    skip_launch: bool,
    skip_bundle: bool,
) -> Result<PathBuf> {
    let app_bin = executable_destination(dest);
    if skip_bundle {
        if !app_bin.is_file() {
            bail!("找不到已安装的程序：{}", app_bin.display());
        }
    } else {
        let src = current_executable()?;
        if let Some(parent) = app_bin.parent() {
            fs::create_dir_all(parent)?;
        }
        if !same_path(&src, &app_bin) {
            if app_bin.exists() {
                close_running_window();
                std::thread::sleep(Duration::from_millis(250));
            }
            fs::copy(&src, &app_bin)
                .with_context(|| format!("复制 {} -> {}", src.display(), app_bin.display()))?;
        }
    }

    install_provider_hooks(paths, &app_bin)?;
    set_autostart(&app_bin)?;

    if !skip_launch {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        Command::new(&app_bin)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .with_context(|| format!("启动 {}", app_bin.display()))?;
    }

    append_log(
        paths,
        &format!(
            "installed windows app={} providers=codex,cursor autostart=true skip_bundle={skip_bundle}",
            app_bin.display()
        ),
    );
    println!("已安装 {}", app_bin.display());
    println!("已配置 Codex 和 Cursor 用户级 Hooks。");
    println!("已为当前 Windows 用户启用登录自启动。");
    Ok(app_bin)
}

/// Package the currently running release executable. The PowerShell build
/// script is the preferred entry point because it can also invoke Inno Setup.
pub fn package(out: PathBuf, zip: bool) -> Result<PathBuf> {
    let src = current_executable()?;
    let dest = if out
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        out
    } else {
        fs::create_dir_all(&out)?;
        out.join(APP_NAME)
    };
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if !same_path(&src, &dest) {
        fs::copy(&src, &dest)
            .with_context(|| format!("复制 {} -> {}", src.display(), dest.display()))?;
    }
    let dest = dest.canonicalize().context("定位 Windows 可执行文件")?;
    println!("已打包 {}", dest.display());

    if zip {
        let zip_path = dest.with_file_name("HarborLight-windows.zip");
        if zip_path.exists() {
            fs::remove_file(&zip_path)?;
        }
        let parent = dest.parent().context("Windows 包缺少父目录")?;
        let status = Command::new("tar.exe")
            .args(["-a", "-c", "-f"])
            .arg(&zip_path)
            .arg("-C")
            .arg(parent)
            .arg(APP_NAME)
            .status()
            .context("使用 Windows tar 创建 zip")?;
        if !status.success() {
            bail!("zip 失败: {}", zip_path.display());
        }
        println!("已压缩 {}", zip_path.display());
    }
    Ok(dest)
}

pub fn uninstall(paths: &Paths, dest: Option<PathBuf>) -> Result<()> {
    remove_autostart();
    close_running_window();
    std::thread::sleep(Duration::from_millis(250));
    let _ = remove_all_provider_hooks(paths);
    let _ = crate::activity::clear_all_activities(paths);
    let _ = fs::remove_file(paths.status_file());
    let _ = fs::remove_file(paths.log_file());
    let _ = fs::remove_file(paths.home.join(".codex-status-light.log"));
    let _ = fs::remove_file(paths.windows_position_file());
    let _ = fs::remove_file(paths.home.join(".codex-light-window-windows.json"));
    let _ = fs::remove_dir_all(paths.light_dir());
    let _ = fs::remove_dir_all(paths.home.join(".codex-light"));

    let app_bin = executable_destination(dest);
    if app_bin.exists() {
        let current = current_executable()?;
        if same_path(&current, &app_bin) {
            schedule_self_delete(&app_bin)?;
        } else {
            fs::remove_file(&app_bin).with_context(|| format!("无法删除 {}", app_bin.display()))?;
            remove_empty_parent(&app_bin);
        }
    }
    println!("已卸载 Harbor Light。若从安装器安装，Windows 会继续清理安装目录。");
    Ok(())
}

fn close_running_window() {
    for class_name in [WINDOW_CLASS_NAME, LEGACY_WINDOW_CLASS_NAME] {
        let class = wide(class_name);
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
            if !hwnd.is_null() {
                let _ = PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

fn schedule_self_delete(binary: &Path) -> Result<()> {
    let parent = binary.parent().unwrap_or_else(|| Path::new("."));
    // Pass paths through dedicated environment variables instead of embedding
    // them into PowerShell source, so custom paths cannot become commands.
    let script = concat!(
        "Start-Sleep -Seconds 2; ",
        "Remove-Item -LiteralPath $env:HARBOR_LIGHT_DELETE_EXE -Force -ErrorAction SilentlyContinue; ",
        "Remove-Item -LiteralPath $env:HARBOR_LIGHT_DELETE_DIR -Force -ErrorAction SilentlyContinue"
    );
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .env("HARBOR_LIGHT_DELETE_EXE", binary)
        .env("HARBOR_LIGHT_DELETE_DIR", parent)
        .spawn()
        .context("安排卸载后的自删除")?;
    Ok(())
}

fn remove_empty_parent(binary: &Path) {
    if let Some(parent) = binary.parent() {
        let _ = fs::remove_dir(parent);
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_accepts_directory_or_exe() {
        assert_eq!(
            executable_destination(Some(PathBuf::from(r"C:\Tools"))),
            PathBuf::from(r"C:\Tools\HarborLight.exe")
        );
        assert_eq!(
            executable_destination(Some(PathBuf::from(r"C:\Tools\light.exe"))),
            PathBuf::from(r"C:\Tools\light.exe")
        );
    }

    #[test]
    fn autostart_path_is_quoted_without_literal_backslashes() {
        assert_eq!(
            autostart_value(Path::new(r"C:\Users\A B\HarborLight.exe")),
            r#""C:\Users\A B\HarborLight.exe""#
        );
    }
}
