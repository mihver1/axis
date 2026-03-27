use crate::registry::DaemonRegistry;
use anyhow::Context;
use std::fs;
use std::path::{Path, PathBuf};

const WORKDESKS_FILE: &str = "workdesks.json";

pub fn registry_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(WORKDESKS_FILE)
}

pub fn load_registry(data_dir: &Path) -> anyhow::Result<DaemonRegistry> {
    let path = registry_file_path(data_dir);
    let payload = match fs::read(&path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DaemonRegistry::new())
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };

    serde_json::from_slice(&payload).with_context(|| format!("parse {}", path.display()))
}

pub fn save_registry(data_dir: &Path, registry: &DaemonRegistry) -> anyhow::Result<()> {
    fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let path = registry_file_path(data_dir);
    let payload = serde_json::to_vec_pretty(registry)
        .with_context(|| format!("serialize {}", path.display()))?;
    fs::write(&path, payload).with_context(|| format!("write {}", path.display()))
}
