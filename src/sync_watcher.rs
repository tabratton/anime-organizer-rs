use crate::{PathConfig, copy_file};
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{error, info};
use walkdir::{DirEntry, WalkDir};

pub struct SyncWatcher(PathConfig);

impl SyncWatcher {
    pub fn new(config: PathConfig) -> Self {
        Self(config)
    }

    pub async fn start(&self) -> Result<(), anyhow::Error> {
        info!("Starting {} thread. Beginning sync", self.0.name);

        self.sync_dirs();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = event.unwrap();
                match event.kind {
                    notify::EventKind::Create(_) | notify::EventKind::Remove(_) => {
                        tx.send(event).unwrap()
                    }
                    _ => {}
                }
            })?;

        let path = self.0.source.clone();
        watcher.watch(Path::new(&path), RecursiveMode::Recursive)?;
        while let Some(event) = rx.recv().await {
            match event.kind {
                notify::EventKind::Create(_) => self.copy_file(event.paths).await,
                notify::EventKind::Remove(_) => self.delete_file(event.paths),
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    async fn copy_file(&self, paths: Vec<PathBuf>) {
        for path in paths {
            let destination_name = self
                .0
                .destination
                .join(path.strip_prefix(&self.0.source).unwrap());
            let file_name = path.file_name().unwrap();
            info!("Copying {file_name:?} to {destination_name:?}");
            std::fs::create_dir_all(destination_name.parent().unwrap()).unwrap();
            match copy_file(path.clone(), destination_name).await {
                Ok(_) => info!("Copied {file_name:?}"),
                Err(error) => error!(error = %error, "Error while copying {file_name:?}"),
            }
        }
    }

    fn delete_file(&self, paths: Vec<PathBuf>) {
        for path in paths {
            let destination_path = self
                .0
                .destination
                .join(path.strip_prefix(&self.0.source).unwrap());
            let file_name = path.file_name().unwrap();
            info!("Deleting {file_name:?}");
            match std::fs::remove_file(&destination_path) {
                Ok(_) => {
                    let parent = destination_path.parent().unwrap();
                    let file_count = std::fs::read_dir(parent).unwrap().count();
                    if file_count == 0 {
                        std::fs::remove_dir(parent).unwrap();
                    }
                    info!("Removed {file_name:?}");
                }
                Err(error) => error!(error = %error, "Error deleting file {file_name:?}"),
            }
        }
    }

    fn sync_dirs(&self) {
        std::fs::create_dir_all(&self.0.source).unwrap();
        std::fs::create_dir_all(&self.0.destination).unwrap();

        let source_list = Self::scan_dir(&self.0.source);
        let destination_list = Self::scan_dir(&self.0.destination);

        SyncWatcher::remove_difference(&destination_list, &source_list);
        SyncWatcher::remove_difference(&source_list, &destination_list);

        info!("Done syncing");
    }

    fn remove_difference(base: &HashSet<FileCompare>, other: &HashSet<FileCompare>) {
        base.difference(other).for_each(|fc| {
            if fc.path.is_dir() {
                std::fs::remove_dir_all(&fc.path).unwrap()
            } else {
                std::fs::remove_file(&fc.path).unwrap()
            }
        });
    }

    fn scan_dir(dir: &PathBuf) -> HashSet<FileCompare> {
        let mut set = HashSet::new();
        for entry in WalkDir::new(dir) {
            let entry = match entry {
                Ok(entry) => {
                    if !entry.file_type().is_file() {
                        continue;
                    }

                    entry
                }
                Err(err) => {
                    error!("error for {err}");
                    continue;
                }
            };

            set.insert(entry.into());
        }

        set
    }
}

impl From<PathConfig> for SyncWatcher {
    fn from(path: PathConfig) -> SyncWatcher {
        Self(path)
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
struct FileCompare {
    path: PathBuf,
    size: u64,
}

impl From<DirEntry> for FileCompare {
    fn from(entry: DirEntry) -> FileCompare {
        Self {
            path: entry.path().to_path_buf(),
            size: entry.metadata().unwrap().len(),
        }
    }
}
