#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod activity;
mod cli;
mod hook;
mod install;
mod paths;
mod providers;
mod sessions;
mod status;

#[cfg(target_os = "macos")]
mod app;

#[cfg(target_os = "windows")]
#[path = "app/windows.rs"]
mod app;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod app {
    pub fn run() -> anyhow::Result<()> {
        anyhow::bail!("Harbor Light GUI 目前支持 macOS 和 Windows");
    }
}

fn main() {
    let cli = <cli::Cli as clap::Parser>::parse();
    if let Err(err) = cli::execute(cli) {
        // Hook must stay quiet on stdout; everything else may print.
        let is_hook = std::env::args().nth(1).is_some_and(|a| a == "hook");
        if is_hook {
            let paths = paths::Paths::current();
            paths::append_log(&paths, &format!("fatal: {err:#}"));
            std::process::exit(0);
        }
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}
