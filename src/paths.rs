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

    pub(crate) fn journal_index(&self) -> PathBuf {
        self.journal_root().join("index.json")
    }

    pub(crate) fn journal_daily_dir(&self) -> PathBuf {
        self.journal_root().join("daily")
    }

    pub(crate) fn journal_weekly_dir(&self) -> PathBuf {
        self.journal_root().join("weekly")
    }

    pub(crate) fn journal_monthly_dir(&self) -> PathBuf {
        self.journal_root().join("monthly")
    }

    pub(crate) fn daily_file(&self, date: chrono::NaiveDate) -> PathBuf {
        self.journal_daily_dir()
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    pub(crate) fn weekly_file(&self, year: i32, week: u32) -> PathBuf {
        self.journal_weekly_dir()
            .join(format!("{year}-W{week:02}.md"))
    }

    pub(crate) fn monthly_file(&self, year: i32, month: u32) -> PathBuf {
        self.journal_monthly_dir()
            .join(format!("{year}-{month:02}.md"))
    }

    pub(crate) fn ensure_journal_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.journal_daily_dir())?;
        fs::create_dir_all(self.journal_weekly_dir())?;
        fs::create_dir_all(self.journal_monthly_dir())?;
        Ok(())
    }

    fn journal_root(&self) -> PathBuf {
        self.root.join("journal")
    }
}
