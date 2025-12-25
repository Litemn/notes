use anyhow::{anyhow, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

pub struct DataPaths {
    pub root: PathBuf,
    pub versions: PathBuf,
    pub files: PathBuf,
    pub index: PathBuf,
    pub daemon_pid: PathBuf,
    pub daemon_log: PathBuf,
}

impl DataPaths {
    pub(crate) fn new() -> Result<Self> {
        let root = match env::var("NOTES_HOME") {
            Ok(path) => PathBuf::from(path),
            Err(_) => dirs::home_dir()
                .map(|p| p.join(".notes"))
                .ok_or_else(|| anyhow!("Unable to determine home directory"))?,
        };

        Ok(Self {
            index: root.join("index.json"),
            versions: root.join("versions"),
            files: root.join("files"),
            daemon_pid: root.join("daemon.pid"),
            daemon_log: root.join("daemon.log"),
            root,
        })
    }

    pub(crate) fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.versions)?;
        fs::create_dir_all(&self.files)?;
        Ok(())
    }

    pub(crate) fn working_file(&self, slug: &str) -> PathBuf {
        self.files.join(format!("{slug}.md"))
    }
}
