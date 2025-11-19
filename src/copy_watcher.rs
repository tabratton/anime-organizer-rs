use crate::{DETECTED_FILES, MOVED_FILES, PathConfig, copy_file};
use anitomy::ElementKind;
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info};

pub struct CopyWatcher(PathConfig);

impl CopyWatcher {
    pub fn new(config: PathConfig) -> Self {
        Self(config)
    }

    pub async fn start(&self) -> Result<(), anyhow::Error> {
        info!("Starting {} thread", self.0.name);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = event.unwrap();
                if let notify::EventKind::Create(_) = event.kind {
                    tx.send(event).unwrap();
                }
            })?;

        let path = self.0.source.clone();
        watcher.watch(Path::new(&path), RecursiveMode::Recursive)?;
        while let Some(event) = rx.recv().await {
            match event.kind {
                notify::EventKind::Create(_) => self.copy_file(event.paths).await,
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    async fn copy_file(&self, paths: Vec<PathBuf>) {
        let mut detected_files = DETECTED_FILES.lock().await;
        for path in paths {
            if path.ends_with(".partial") || detected_files.contains(&path) {
                continue;
            }

            detected_files.insert(path.clone());
            self.spawn_mover(path);
        }
    }

    fn spawn_mover(&self, path: PathBuf) {
        info!("{} found, moving to correct folder", path.display());
        let mover = Mover::new(self.0.destination.clone(), path, self.0.place_in_sub);
        tokio::spawn(async move {
            mover.start().await;
        });
    }
}

impl From<PathConfig> for CopyWatcher {
    fn from(path: PathConfig) -> CopyWatcher {
        Self(path)
    }
}

struct Mover {
    destination: PathBuf,
    detected_file: PathBuf,
    subfolder: bool,
    wait_time: Duration,
    title: Option<String>,
}

impl Mover {
    fn new(destination: PathBuf, detected_file: PathBuf, subfolder: bool) -> Self {
        let title = get_title(&detected_file);
        let wait_time = Duration::from_secs(5);
        Self {
            destination,
            detected_file,
            subfolder,
            wait_time,
            title,
        }
    }

    async fn start(&self) {
        let destination = self.setup_destination_folder();
        self.perform_move(destination).await;
    }

    fn setup_destination_folder(&self) -> PathBuf {
        let mut destination = PathBuf::new();

        if let Some(title) = &self.title
            && self.subfolder
        {
            let mut folder = PathBuf::from(&self.destination);
            folder.push(title);
            create_folder(&folder);
            destination.push(&folder);
            destination.push(self.detected_file.file_name().unwrap());
        } else {
            create_folder(&self.destination);
            destination.push(&self.destination);
            destination.push(self.detected_file.file_name().unwrap());
        }

        destination
    }

    async fn perform_move(&self, destination: PathBuf) {
        let mut file_moved = false;
        while !file_moved {
            tokio::time::sleep(self.wait_time).await;

            if is_downloading(&self.detected_file) {
                continue;
            }

            info!("Starting copy {}", self.detected_file.display());

            if self.detected_file.is_dir() {
                match copy_dir_all(&self.detected_file, &destination).await {
                    Ok(_) => file_moved = true,
                    Err(e) => error!(
                        "Error copying {} to {}: {}",
                        self.detected_file.display(),
                        destination.display(),
                        e
                    ),
                }
            } else {
                match copy_file(self.detected_file.clone(), destination.clone()).await {
                    Ok(_) => file_moved = true,
                    Err(e) => error!(
                        "Error copying {} to {}: {}",
                        self.detected_file.display(),
                        destination.display(),
                        e
                    ),
                }
            }
        }

        info!("{} moved successfully", self.detected_file.display());
        MOVED_FILES.lock().await.insert(destination);
    }
}

fn get_title(detected_file: &Path) -> Option<String> {
    anitomy::parse(detected_file.file_name().unwrap().to_str().unwrap())
        .iter()
        .find(|element| element.kind() == ElementKind::Title)
        .map(|element| element.value().to_string())
}

fn create_folder(folder: &Path) {
    if folder.exists() {
        return;
    }

    match std::fs::create_dir_all(folder) {
        Ok(_) => {}
        Err(e) => {
            error!("Could not create folder {}: {}", folder.display(), e);
        }
    }
}

async fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            Box::pin(copy_dir_all(
                entry.path(),
                dst.as_ref().join(entry.file_name()),
            ))
            .await?;
        } else {
            copy_file(entry.path(), dst.as_ref().join(entry.file_name())).await?;
        }
    }
    Ok(())
}

fn is_downloading(file: &Path) -> bool {
    if file.is_dir() {
        std::fs::read_dir(file)
            .unwrap()
            .map(|e| e.unwrap())
            .any(|e| e.path().ends_with(".partial"))
            || std::fs::read_dir(file)
                .unwrap()
                .map(|e| e.unwrap())
                .filter(|e| e.file_type().unwrap().is_dir())
                .any(|e| is_downloading(e.path().as_path()))
    } else {
        false
    }
}
