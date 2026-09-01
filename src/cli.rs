use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::install;
use crate::paths::Paths;
use crate::providers::Provider;
use crate::status::{read_current, write_current, LightState, StatusSnapshot};

#[derive(Parser, Debug)]
#[command(name = "harbor-light", about = "可跨屏拖动的 AI 编程工具红绿灯状态悬浮窗")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 编程软件的 hooks.json 调用：从 stdin 读事件并更新活动状态
    Hook {
        /// 事件来源；旧命令未传入时保持为 Codex
        #[arg(long, value_enum, default_value_t = Provider::Codex)]
        provider: Provider,
    },
    /// 打印 ~/.codex-status.json
    Status,
    /// 手动写入状态（用于联调四态动画）
    Set {
        #[arg(value_enum)]
        state: LightState,
    },
    /// 安装程序、合并 hooks、配置登录自启动
    Install {
        /// 安装路径；macOS 为 .app，Windows 为 .exe 或目标目录
        #[arg(long)]
        dest: Option<PathBuf>,
        /// 配置文件和自启动，但不立即启动 App
        #[arg(long)]
        skip_launch: bool,
        /// 不复制程序，只配置 hooks / 自启动（给系统安装器用）
        #[arg(long)]
        skip_bundle: bool,
    },
    /// 打包当前平台的可分发程序（不装 hook、不开机自启）
    Package {
        /// 输出目录、.app 路径或 .exe 路径，默认 dist/
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        /// 同时打 zip
        #[arg(long)]
        zip: bool,
    },
    /// 停止 App、移除登录自启动、清理本工具写入的 hook
    Uninstall {
        #[arg(long)]
        dest: Option<PathBuf>,
    },
}

pub fn execute(cli: Cli) -> anyhow::Result<()> {
    let paths = Paths::current();
    match cli.command {
        None => crate::app::run(),
        Some(Command::Hook { provider }) => crate::hook::run(&paths, provider, std::io::stdin()),
        Some(Command::Status) => {
            match read_current(&paths)? {
                Some(snap) => {
                    println!("{}", serde_json::to_string_pretty(&snap)?);
                }
                None => {
                    println!("(no status file at {})", paths.status_file().display());
                }
            }
            Ok(())
        }
        Some(Command::Set { state }) => {
            let snap = StatusSnapshot::new(state, "cli", None, None);
            write_current(&paths, &snap)?;
            println!("已写入 {} => {state}", paths.status_file().display());
            Ok(())
        }
        Some(Command::Install {
            dest,
            skip_launch,
            skip_bundle,
        }) => {
            install::install(&paths, dest, skip_launch, skip_bundle)?;
            Ok(())
        }
        Some(Command::Package { out, zip }) => {
            install::package(out, zip)?;
            Ok(())
        }
        Some(Command::Uninstall { dest }) => install::uninstall(&paths, dest),
    }
}
