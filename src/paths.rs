use anyhow::{Context, Result};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct AppDirs {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}
impl AppDirs {
    pub fn discover(override_root: Option<&Path>) -> Result<Self> {
        if let Some(root) = override_root {
            let root = if root.is_absolute() {
                root.to_owned()
            } else {
                std::env::current_dir()?.join(root)
            };
            return Ok(Self {
                config: root.join("config"),
                cache: root.join("cache"),
                data: root,
            });
        }
        let dirs = directories::ProjectDirs::from("local", "ReadLater", "Read Later")
            .context("Cannot locate native application storage")?;
        let data = dirs.data_local_dir().to_owned();
        Ok(Self {
            config: data.join("config"),
            cache: data.join("cache"),
            data,
        })
    }
    pub fn ensure(&self) -> Result<()> {
        for p in [&self.data, &self.config, &self.cache] {
            if p.symlink_metadata().is_ok() {
                let metadata = p.symlink_metadata()?;
                anyhow::ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "Private storage must be a regular directory"
                );
            }
            std::fs::create_dir_all(p)?;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    pub fn database(&self) -> PathBuf {
        self.data.join("library.sqlite3")
    }
    pub fn settings(&self) -> PathBuf {
        self.config.join("reader.json")
    }
}
