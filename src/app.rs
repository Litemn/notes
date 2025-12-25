use crate::paths::DataPaths;
use crate::utils::{hash_bytes, slugify};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Index {
    notes: HashMap<String, NoteMeta>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NoteMeta {
    title: String,
    slug: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    current_version: u32,
    versions: Vec<VersionMeta>,
    working_hash: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct VersionMeta {
    version: u32,
    path: String,
    hash: String,
    created_at: DateTime<Utc>,
}

pub struct NotesApp {
    paths: DataPaths,
    index: Index,
}

impl NotesApp {
    pub fn load() -> Result<Self> {
        let paths = DataPaths::new()?;
        paths.ensure_dirs()?;

        let index = if paths.index.exists() {
            let content = fs::read_to_string(&paths.index)
                .with_context(|| format!("Failed to read {}", paths.index.display()))?;
            serde_json::from_str::<Index>(&content)
                .with_context(|| format!("Failed to parse {}", paths.index.display()))?
        } else {
            Index::default()
        };

        Ok(Self { paths, index })
    }

    pub fn save(&self) -> Result<()> {
        let serialized = serde_json::to_string_pretty(&self.index)?;
        fs::write(&self.paths.index, serialized)
            .with_context(|| format!("Failed to write {}", self.paths.index.display()))
    }

    pub fn paths(&self) -> &DataPaths {
        &self.paths
    }

    pub fn create_note(&mut self, title: Option<String>) -> Result<PathBuf> {
        let now = Utc::now();
        let title = title.unwrap_or_else(|| format!("note-{}", now.format("%Y%m%d-%H%M%S")));
        let mut slug = slugify(&title);

        let mut counter = 1;
        while self.index.notes.contains_key(&slug) {
            counter += 1;
            slug = format!("{}-{}", slugify(&title), counter);
        }

        let note_dir = self.paths.versions.join(&slug);
        fs::create_dir_all(&note_dir)
            .with_context(|| format!("Failed to create {}", note_dir.display()))?;

        let version_number = 1;
        let version_path_rel = format!("versions/{}/{:07}.md", slug, version_number);
        let version_path = self.paths.root.join(&version_path_rel);
        fs::create_dir_all(
            version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&version_path, b"")?;

        let working_path = self.paths.working_file(&slug);
        fs::create_dir_all(
            working_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid working path"))?,
        )?;
        fs::write(&working_path, b"")?;

        let hash = hash_bytes(b"");
        let version = VersionMeta {
            version: version_number,
            path: version_path_rel,
            hash: hash.clone(),
            created_at: now,
        };

        let meta = NoteMeta {
            title: title.clone(),
            slug: slug.clone(),
            created_at: now,
            updated_at: now,
            current_version: version_number,
            versions: vec![version],
            working_hash: Some(hash),
        };

        self.index.notes.insert(slug.clone(), meta);
        Ok(working_path)
    }

    pub fn open_note(&mut self, identifier: &str) -> Result<PathBuf> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        self.snapshot_if_changed(&slug)?;

        Ok(self.paths.working_file(&slug))
    }

    pub fn list_notes(&self) -> Result<()> {
        if self.index.notes.is_empty() {
            println!("No notes yet. Run `notes new` to create one.");
            return Ok(());
        }

        let mut notes: Vec<&NoteMeta> = self.index.notes.values().collect();
        notes.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

        for note in notes {
            let path = self.paths.working_file(&note.slug);
            println!(
                "- {} (id: {}) versions: {} current: {} path: {}",
                note.title,
                note.slug,
                note.versions.len(),
                note.current_version,
                path.display()
            );
        }

        Ok(())
    }

    pub fn list_versions(&mut self, identifier: &str) -> Result<()> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;
        let note = self
            .index
            .notes
            .get(&slug)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        println!("Versions for {}:", note.title);
        for version in &note.versions {
            println!(
                "  v{} @ {} ({})",
                version.version,
                version.created_at.to_rfc3339(),
                version.path
            );
        }

        Ok(())
    }

    pub fn list_ids(&self) -> Result<()> {
        let mut ids: Vec<&String> = self.index.notes.keys().collect();
        ids.sort();
        for id in ids {
            println!("{}", id);
        }
        Ok(())
    }

    pub fn rollback(&mut self, identifier: &str, target_version: Option<u32>) -> Result<PathBuf> {
        let slug = self
            .resolve_slug(identifier)
            .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

        self.snapshot_if_changed(&slug)?;

        let (target, current_version) = {
            let note = self
                .index
                .notes
                .get(&slug)
                .ok_or_else(|| anyhow!("Note not found: {}", identifier))?;

            let desired = match target_version {
                Some(v) => v,
                None => note.current_version.saturating_sub(1),
            };

            if desired == 0 {
                bail!("No previous version to roll back to");
            }

            let target = note
                .versions
                .iter()
                .find(|v| v.version == desired)
                .ok_or_else(|| anyhow!("Version {} not found", desired))?
                .clone();

            (target, note.current_version)
        };

        let content = fs::read(self.paths.root.join(&target.path))
            .with_context(|| format!("Failed to read {}", target.path))?;

        let hash = hash_bytes(&content);
        let new_version_number = current_version + 1;
        let new_version_rel = format!("versions/{}/{:07}.md", slug, new_version_number);
        let new_version_path = self.paths.root.join(&new_version_rel);
        fs::create_dir_all(
            new_version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&new_version_path, &content)?;

        let now = Utc::now();
        let new_meta = VersionMeta {
            version: new_version_number,
            path: new_version_rel,
            hash: hash.clone(),
            created_at: now,
        };

        if let Some(note) = self.index.notes.get_mut(&slug) {
            note.versions.push(new_meta);
            note.current_version = new_version_number;
            note.updated_at = now;
            note.working_hash = Some(hash);
        }

        let working_path = self.paths.working_file(&slug);
        fs::write(&working_path, &content)?;

        Ok(working_path)
    }

    pub fn delete_note_by_title(&mut self, title: &str) -> Result<String> {
        let slug = self.resolve_unique_title_slug(title)?;

        let note = self
            .index
            .notes
            .remove(&slug)
            .ok_or_else(|| anyhow!("Note not found: {}", title))?;

        let working_path = self.paths.working_file(&slug);
        if working_path.exists() {
            fs::remove_file(&working_path)
                .with_context(|| format!("Failed to remove {}", working_path.display()))?;
        }

        let versions_dir = self.paths.versions.join(&slug);
        if versions_dir.exists() {
            fs::remove_dir_all(&versions_dir)
                .with_context(|| format!("Failed to remove {}", versions_dir.display()))?;
        }

        Ok(note.slug)
    }

    pub fn search(&mut self, query: &str) -> Result<()> {
        let needle = query.to_lowercase();
        let mut matches_found = false;

        let mut notes: Vec<&NoteMeta> = self.index.notes.values().collect();
        notes.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

        for note in notes {
            let content = fs::read_to_string(self.current_version_path(note))
                .unwrap_or_else(|_| String::new());
            if content.to_lowercase().contains(&needle) {
                matches_found = true;
                println!("- {} (id: {})", note.title, note.slug);
            }
        }

        if !matches_found {
            println!("No matches found.");
        }

        Ok(())
    }

    pub fn snapshot_all_changes(&mut self) -> Result<Vec<String>> {
        let slugs: Vec<String> = self.index.notes.keys().cloned().collect();
        let mut updated = Vec::new();
        for slug in slugs {
            if self.snapshot_if_changed(&slug)? {
                updated.push(slug);
            }
        }
        Ok(updated)
    }

    pub fn snapshot_if_changed(&mut self, slug: &str) -> Result<bool> {
        self.ensure_working_copy_exists(slug)?;
        let note = self
            .index
            .notes
            .get_mut(slug)
            .ok_or_else(|| anyhow!("Note not found: {}", slug))?;
        let working_path = self.paths.working_file(slug);
        let content = fs::read(&working_path)
            .with_context(|| format!("Failed to read {}", working_path.display()))?;
        let hash = hash_bytes(&content);

        if let Some(last) = note.versions.last() {
            if last.hash == hash {
                note.working_hash = Some(hash);
                return Ok(false);
            }
        }

        let new_version_number = note.current_version + 1;
        let version_rel = format!("versions/{}/{:07}.md", slug, new_version_number);
        let version_path = self.paths.root.join(&version_rel);
        fs::create_dir_all(
            version_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid version path"))?,
        )?;
        fs::write(&version_path, &content)?;

        let now = Utc::now();
        let meta = VersionMeta {
            version: new_version_number,
            path: version_rel,
            hash: hash.clone(),
            created_at: now,
        };

        note.versions.push(meta);
        note.current_version = new_version_number;
        note.updated_at = now;
        note.working_hash = Some(hash);

        Ok(true)
    }

    fn ensure_working_copy_exists(&self, slug: &str) -> Result<()> {
        let working_path = self.paths.working_file(slug);
        if working_path.exists() {
            return Ok(());
        }

        let note = self
            .index
            .notes
            .get(slug)
            .ok_or_else(|| anyhow!("Note not found: {}", slug))?;
        let source = self.current_version_path(note);
        let content =
            fs::read(&source).with_context(|| format!("Failed to read {}", source.display()))?;
        fs::create_dir_all(
            working_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid working path"))?,
        )?;
        fs::write(&working_path, content)?;

        Ok(())
    }

    fn resolve_slug(&self, identifier: &str) -> Option<String> {
        if self.index.notes.contains_key(identifier) {
            return Some(identifier.to_string());
        }

        let id_lower = identifier.to_lowercase();
        self.index
            .notes
            .values()
            .find(|note| note.title.to_lowercase() == id_lower || note.slug == id_lower)
            .map(|note| note.slug.clone())
    }

    fn resolve_unique_title_slug(&self, title: &str) -> Result<String> {
        let matches: Vec<&NoteMeta> = self
            .index
            .notes
            .values()
            .filter(|note| note.slug.to_lowercase() == title)
            .collect();

        match matches.as_slice() {
            [] => bail!("Note not found: {}", title),
            [note] => Ok(note.slug.clone()),
            _ => bail!("Multiple notes match title: {}", title),
        }
    }

    fn current_version_path(&self, note: &NoteMeta) -> PathBuf {
        if let Some(version) = note
            .versions
            .iter()
            .find(|v| v.version == note.current_version)
        {
            return self.paths.root.join(&version.path);
        }

        self.paths
            .root
            .join(&note.versions.last().expect("note has versions").path)
    }
}
