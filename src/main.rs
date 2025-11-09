mod copy_watcher;
mod sync_watcher;

use crate::copy_watcher::CopyWatcher;
use crate::sync_watcher::SyncWatcher;
use lazy_static::lazy_static;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

#[derive(Deserialize)]
struct Config {
    paths: Vec<PathConfig>,
}

#[derive(Deserialize)]
struct PathConfig {
    source: PathBuf,
    destination: PathBuf,
    place_in_sub: bool,
    name: String,
    watcher_type: WatcherTypeConfig,
}

fn setup_logging() {
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::fmt::time::UtcTime;
    use tracing_subscriber::fmt::{format, layer};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Registry};

    Registry::default()
        .with(EnvFilter::from_default_env())
        .with(
            layer()
                .event_format(format().with_timer(UtcTime::rfc_3339()))
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_span_events(FmtSpan::CLOSE)
                .with_level(true)
                .with_thread_ids(true)
                .with_thread_names(true),
        )
        .init();
}

lazy_static! {
    static ref DETECTED_FILES: Mutex<HashSet<PathBuf>> = Mutex::new(HashSet::new());
}

lazy_static! {
    static ref MOVED_FILES: Mutex<HashSet<PathBuf>> = Mutex::new(HashSet::new());
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    setup_logging();

    let config_file = tokio::fs::read_to_string("./paths.toml").await?;
    let config: Config = toml::from_str(&config_file)?;

    let mut join_set = JoinSet::new();
    for path_config in config.paths {
        join_set.spawn(async move {
            let watcher: FileWatcherType = match &path_config.watcher_type {
                WatcherTypeConfig::Sync => FileWatcherType::Sync(SyncWatcher::new(path_config)),
                WatcherTypeConfig::Copy => FileWatcherType::Copy(CopyWatcher::new(path_config)),
            };
            watcher.start().await.expect("TODO: panic message");
        });
    }

    join_set.join_all().await;
    Ok(())
}

trait FileWatcher {
    async fn start(&self) -> Result<(), anyhow::Error>;
}

enum FileWatcherType {
    Sync(SyncWatcher),
    Copy(CopyWatcher),
}

impl FileWatcher for FileWatcherType {
    async fn start(&self) -> Result<(), anyhow::Error> {
        match self {
            FileWatcherType::Sync(sync_watcher) => sync_watcher.start().await,
            FileWatcherType::Copy(copy_watcher) => copy_watcher.start().await,
        }
    }
}

#[derive(Debug, Deserialize)]
enum WatcherTypeConfig {
    // Adds files from source to dest, removes files not present in source from dest, then watches source for further changes
    Sync,
    // Watches source and copies files to dest
    Copy,
}

async fn copy_file(source: PathBuf, destination: PathBuf) -> std::io::Result<u64> {
    tokio::task::spawn_blocking(move || std::fs::copy(source, destination)).await?
}
