use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::paths::Paths;

use super::NEEDS_REFRESH;

pub fn spawn_watcher() {
    thread::Builder::new()
        .name("harbor-light-watch".into())
        .spawn(|| {
            if let Err(err) = run_watcher() {
                crate::paths::append_log(&Paths::current(), &format!("watcher exited: {err:#}"));
            }
        })
        .ok();
}

fn run_watcher() -> notify::Result<()> {
    let paths = Paths::current();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    let status_parent = paths
        .status_file()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| paths.home.clone());
    let _ = watcher.watch(&status_parent, RecursiveMode::NonRecursive);
    if paths.activities_dir().exists() {
        let _ = watcher.watch(&paths.activities_dir(), RecursiveMode::Recursive);
    }
    if paths.sessions_dir().exists() {
        let _ = watcher.watch(&paths.sessions_dir(), RecursiveMode::Recursive);
    } else if paths.codex_dir().exists() {
        let _ = watcher.watch(&paths.codex_dir(), RecursiveMode::NonRecursive);
    }

    NEEDS_REFRESH.store(true, Ordering::SeqCst);

    while let Ok(event) = rx.recv() {
        let Ok(_event) = event else {
            continue;
        };
        // FSEvents may classify an atomic replace as `Other`; every successful
        // event from our narrowly scoped watches is relevant enough to refresh.
        NEEDS_REFRESH.store(true, Ordering::SeqCst);
        // Coalesce bursts of FSEvents.
        thread::sleep(Duration::from_millis(30));
        while rx.try_recv().is_ok() {}
    }
    Ok(())
}
