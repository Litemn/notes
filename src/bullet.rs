use crate::cli::BulletAction;
use crate::paths::DataPaths;
use crate::utils::launch_subl_if_installed;
use anyhow::{bail, Context, Result};
use chrono::{Datelike, DateTime, NaiveDate, Utc};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Number of days to search back when completing tasks
const TASK_COMPLETION_SEARCH_DAYS: i64 = 30;

/// Number of days to search back when searching journal entries
const JOURNAL_SEARCH_DAYS: i64 = 90;

/// Number of days to look back for listing entry IDs (shell completion)
const ENTRY_IDS_SEARCH_DAYS: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum BulletType {
    Task,
    Event,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum TaskState {
    Incomplete,
    Complete,
    Migrated,
    Scheduled,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct BulletEntry {
    id: String,
    bullet_type: BulletType,
    task_state: Option<TaskState>,
    content: String,
    created_at: DateTime<Utc>,
    date: NaiveDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum JournalPeriod {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JournalEntryRef {
    date_key: String,
    bullet_type: BulletType,
    task_state: Option<TaskState>,
    content_preview: String,
}

#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JournalIndex {
    entries: HashMap<String, JournalEntryRef>,
}

struct BulletJournal {
    paths: DataPaths,
    index: JournalIndex,
}

pub fn handle_bullet_command(
    action: Option<BulletAction>,
    text: Vec<String>,
    task: bool,
    event: bool,
    note: bool,
    date: Option<String>,
    weekly: bool,
    monthly: bool,
) -> Result<()> {
    let paths = DataPaths::new()?;
    let mut journal = BulletJournal::load(paths)?;

    if let Some(action) = action {
        match action {
            BulletAction::List {
                date: list_date,
                week,
                month,
            } => {
                let today = Utc::now().date_naive();
                let entries = if week {
                    let week_num = today.iso_week().week();
                    let year = today.iso_week().year();
                    journal.list_weekly(year, week_num)?
                } else if month {
                    journal.list_monthly(today.year(), today.month())?
                } else {
                    let target_date =
                        list_date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);
                    journal.list_daily(target_date)?
                };
                print_entries(&entries);
            }
            BulletAction::Pending { days } => {
                let entries = journal.list_pending(days)?;
                print_entries(&entries);
            }
            BulletAction::Complete { entry } => {
                journal.complete_task(&entry)?;
                println!("Marked complete: {}", entry);
            }
            BulletAction::Migrate { all, from } => {
                let today = Utc::now().date_naive();
                let from_date = from
                    .map(|d| parse_date(&d))
                    .transpose()?
                    .unwrap_or(today - chrono::Duration::days(1));
                let migrated = journal.migrate_tasks(from_date, all)?;
                if migrated.is_empty() {
                    println!("No tasks to migrate.");
                } else {
                    println!("Migrated {} task(s) to today.", migrated.len());
                }
            }
            BulletAction::Open {
                date: open_date,
                weekly: open_weekly,
                monthly: open_monthly,
            } => {
                let today = Utc::now().date_naive();
                let target_date =
                    open_date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);
                let period = if open_weekly {
                    JournalPeriod::Weekly
                } else if open_monthly {
                    JournalPeriod::Monthly
                } else {
                    JournalPeriod::Daily
                };
                let path = journal.open_file(target_date, period)?;
                launch_subl_if_installed(&path);
                println!("{}", path.display());
            }
            BulletAction::Search { query } => {
                let entries = journal.search(&query)?;
                print_entries(&entries);
            }
            BulletAction::Interactive => {
                journal.run_interactive()?;
                return Ok(());
            }
            BulletAction::Ids => {
                journal.list_ids()?;
                return Ok(());
            }
        }
        journal.save()?;
        return Ok(());
    }

    if !text.is_empty() {
        let content = text.join(" ");
        let today = Utc::now().date_naive();
        let target_date = date.map(|d| parse_date(&d)).transpose()?.unwrap_or(today);

        let bullet_type = if event {
            BulletType::Event
        } else if note {
            BulletType::Note
        } else if task {
            BulletType::Task
        } else {
            BulletType::Task
        };

        let period = if weekly {
            JournalPeriod::Weekly
        } else if monthly {
            JournalPeriod::Monthly
        } else {
            JournalPeriod::Daily
        };

        let id = journal.add_entry(&content, bullet_type, target_date, period)?;
        journal.save()?;

        let type_name = match bullet_type {
            BulletType::Task => "task",
            BulletType::Event => "event",
            BulletType::Note => "note",
        };
        println!("Added {}: {} [{}]", type_name, content, &id[..4.min(id.len())]);
        return Ok(());
    }

    let today = Utc::now().date_naive();
    let entries = journal.list_daily(today)?;
    print_entries(&entries);

    Ok(())
}

pub fn run_interactive() -> Result<()> {
    let paths = DataPaths::new()?;
    let mut journal = BulletJournal::load(paths)?;
    journal.run_interactive()?;
    Ok(())
}

impl BulletJournal {
    fn load(paths: DataPaths) -> Result<Self> {
        paths.ensure_journal_dirs()?;

        let index = if paths.journal_index().exists() {
            let content = fs::read_to_string(paths.journal_index())
                .with_context(|| format!("Failed to read {}", paths.journal_index().display()))?;
            serde_json::from_str::<JournalIndex>(&content).unwrap_or_default()
        } else {
            JournalIndex::default()
        };

        Ok(Self { paths, index })
    }

    fn save(&self) -> Result<()> {
        let serialized = serde_json::to_string_pretty(&self.index)?;
        fs::write(self.paths.journal_index(), serialized)
            .with_context(|| format!("Failed to write {}", self.paths.journal_index().display()))
    }

    fn add_entry(
        &mut self,
        content: &str,
        bullet_type: BulletType,
        date: NaiveDate,
        period: JournalPeriod,
    ) -> Result<String> {
        let id = generate_entry_id();
        let now = Utc::now();

        let task_state = match bullet_type {
            BulletType::Task => Some(TaskState::Incomplete),
            _ => None,
        };

        let preview = if content.len() > 50 {
            format!("{}...", &content[..47])
        } else {
            content.to_string()
        };

        let entry = BulletEntry {
            id: id.clone(),
            bullet_type,
            task_state,
            content: content.to_string(),
            created_at: now,
            date,
        };

        let (file_path, date_key) = match period {
            JournalPeriod::Daily => {
                let path = self.paths.daily_file(date);
                let key = date.format("%Y-%m-%d").to_string();
                (path, key)
            }
            JournalPeriod::Weekly => {
                let week = date.iso_week().week();
                let year = date.iso_week().year();
                let path = self.paths.weekly_file(year, week);
                let key = format!("{}-W{:02}", year, week);
                (path, key)
            }
            JournalPeriod::Monthly => {
                let path = self.paths.monthly_file(date.year(), date.month());
                let key = format!("{}-{:02}", date.year(), date.month());
                (path, key)
            }
        };

        let mut entries = if file_path.exists() {
            let file_content = fs::read_to_string(&file_path)?;
            parse_journal_file(&file_content, date)?
        } else {
            Vec::new()
        };

        entries.push(entry);

        let file_content = format_journal_file(&entries, period, &date_key);
        fs::write(&file_path, &file_content)?;

        self.index.entries.insert(
            id.clone(),
            JournalEntryRef {
                date_key,
                bullet_type,
                task_state,
                content_preview: preview,
            },
        );

        Ok(id)
    }

    fn list_daily(&self, date: NaiveDate) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.daily_file(date);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        parse_journal_file(&content, date)
    }

    fn list_weekly(&self, year: i32, week: u32) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.weekly_file(year, week);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        let date = NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)
            .unwrap_or_else(|| Utc::now().date_naive());
        parse_journal_file(&content, date)
    }

    fn list_monthly(&self, year: i32, month: u32) -> Result<Vec<BulletEntry>> {
        let file_path = self.paths.monthly_file(year, month);
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file_path)?;
        let date = NaiveDate::from_ymd_opt(year, month, 1)
            .unwrap_or_else(|| Utc::now().date_naive());
        parse_journal_file(&content, date)
    }

    fn list_pending(&self, days_back: u32) -> Result<Vec<BulletEntry>> {
        let today = Utc::now().date_naive();
        let mut pending = Vec::new();

        for i in 0..days_back {
            let date = today - chrono::Duration::days(i as i64);
            let entries = self.list_daily(date)?;
            for entry in entries {
                if entry.bullet_type == BulletType::Task
                    && entry.task_state == Some(TaskState::Incomplete)
                {
                    pending.push(entry);
                }
            }
        }

        Ok(pending)
    }

    fn complete_task(&mut self, partial_id: &str) -> Result<()> {
        let today = Utc::now().date_naive();

        for i in 0..TASK_COMPLETION_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let file_path = self.paths.daily_file(date);
            if !file_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&file_path)?;
            let mut entries = parse_journal_file(&content, date)?;
            let mut found = false;

            for entry in &mut entries {
                if entry.id.starts_with(partial_id) && entry.bullet_type == BulletType::Task {
                    entry.task_state = Some(TaskState::Complete);
                    found = true;

                    if let Some(ref_entry) = self.index.entries.get_mut(&entry.id) {
                        ref_entry.task_state = Some(TaskState::Complete);
                    }
                    break;
                }
            }

            if found {
                let date_key = date.format("%Y-%m-%d").to_string();
                let content = format_journal_file(&entries, JournalPeriod::Daily, &date_key);
                fs::write(&file_path, content)?;
                return Ok(());
            }
        }

        bail!("Entry not found: {}", partial_id)
    }

    fn migrate_tasks(&mut self, from_date: NaiveDate, all: bool) -> Result<Vec<String>> {
        let today = Utc::now().date_naive();
        let from_file = self.paths.daily_file(from_date);

        if !from_file.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&from_file)?;
        let mut from_entries = parse_journal_file(&content, from_date)?;
        let mut migrated_ids = Vec::new();

        let mut tasks_to_migrate: Vec<usize> = Vec::new();
        for (i, entry) in from_entries.iter().enumerate() {
            if entry.bullet_type == BulletType::Task
                && entry.task_state == Some(TaskState::Incomplete)
                && (all || tasks_to_migrate.is_empty())
            {
                tasks_to_migrate.push(i);
            }
        }

        if tasks_to_migrate.is_empty() {
            return Ok(Vec::new());
        }

        let to_file = self.paths.daily_file(today);
        let mut to_entries = if to_file.exists() {
            let content = fs::read_to_string(&to_file)?;
            parse_journal_file(&content, today)?
        } else {
            Vec::new()
        };

        for &idx in &tasks_to_migrate {
            let old_entry = &from_entries[idx];
            let new_id = generate_entry_id();

            let new_entry = BulletEntry {
                id: new_id.clone(),
                bullet_type: BulletType::Task,
                task_state: Some(TaskState::Incomplete),
                content: old_entry.content.clone(),
                created_at: Utc::now(),
                date: today,
            };

            to_entries.push(new_entry);
            migrated_ids.push(new_id);
        }

        for &idx in &tasks_to_migrate {
            from_entries[idx].task_state = Some(TaskState::Migrated);
        }

        let from_key = from_date.format("%Y-%m-%d").to_string();
        let to_key = today.format("%Y-%m-%d").to_string();

        fs::write(
            &from_file,
            format_journal_file(&from_entries, JournalPeriod::Daily, &from_key),
        )?;
        fs::write(
            &to_file,
            format_journal_file(&to_entries, JournalPeriod::Daily, &to_key),
        )?;

        Ok(migrated_ids)
    }

    fn search(&self, query: &str) -> Result<Vec<BulletEntry>> {
        let needle = query.to_lowercase();
        let today = Utc::now().date_naive();
        let mut results = Vec::new();

        for i in 0..JOURNAL_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let file_path = self.paths.daily_file(date);
            if !file_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&file_path)?;
            let entries = parse_journal_file(&content, date)?;

            for entry in entries {
                if entry.content.to_lowercase().contains(&needle) {
                    results.push(entry);
                }
            }
        }

        Ok(results)
    }

    fn open_file(&self, date: NaiveDate, period: JournalPeriod) -> Result<PathBuf> {
        let file_path = match period {
            JournalPeriod::Daily => self.paths.daily_file(date),
            JournalPeriod::Weekly => {
                let week = date.iso_week().week();
                let year = date.iso_week().year();
                self.paths.weekly_file(year, week)
            }
            JournalPeriod::Monthly => self.paths.monthly_file(date.year(), date.month()),
        };

        if !file_path.exists() {
            let date_key = match period {
                JournalPeriod::Daily => date.format("%Y-%m-%d").to_string(),
                JournalPeriod::Weekly => {
                    let week = date.iso_week().week();
                    let year = date.iso_week().year();
                    format!("{}-W{:02}", year, week)
                }
                JournalPeriod::Monthly => format!("{}-{:02}", date.year(), date.month()),
            };
            let content = format_journal_file(&[], period, &date_key);
            fs::write(&file_path, content)?;
        }

        Ok(file_path)
    }

    fn run_interactive(&mut self) -> Result<()> {
        use std::io::{BufRead, Write};

        println!("Bullet Journal Interactive Mode");
        println!("Commands: t <text> = task, e <text> = event, n <text> = note");
        println!("          l = list today, p = pending, x <id> = complete, q = quit");
        println!();

        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();

        loop {
            print!("> ");
            stdout.flush()?;

            let mut input = String::new();
            if stdin.lock().read_line(&mut input)? == 0 {
                break;
            }

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            let today = Utc::now().date_naive();

            match input.chars().next() {
                Some('t') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Task,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added task: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('e') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Event,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added event: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('n') => {
                    let content = input[1..].trim();
                    if !content.is_empty() {
                        let id = self.add_entry(
                            content,
                            BulletType::Note,
                            today,
                            JournalPeriod::Daily,
                        )?;
                        println!("Added note: {} [{}]", content, &id[..4.min(id.len())]);
                    }
                }
                Some('l') => {
                    let entries = self.list_daily(today)?;
                    print_entries(&entries);
                }
                Some('p') => {
                    let entries = self.list_pending(7)?;
                    print_entries(&entries);
                }
                Some('x') => {
                    let id = input[1..].trim();
                    if !id.is_empty() {
                        match self.complete_task(id) {
                            Ok(()) => println!("Marked complete: {}", id),
                            Err(err) => println!("Error: {}", err),
                        }
                    }
                }
                Some('q') | Some('Q') => break,
                _ => {
                    let id =
                        self.add_entry(input, BulletType::Task, today, JournalPeriod::Daily)?;
                    println!("Added task: {} [{}]", input, &id[..4.min(id.len())]);
                }
            }
        }

        self.save()?;
        Ok(())
    }

    fn list_ids(&self) -> Result<()> {
        let today = Utc::now().date_naive();

        for i in 0..ENTRY_IDS_SEARCH_DAYS {
            let date = today - chrono::Duration::days(i);
            let entries = self.list_daily(date)?;
            for entry in entries {
                let content = if entry.content.len() > 40 {
                    format!("{}...", &entry.content[..37])
                } else {
                    entry.content.clone()
                };
                println!("{}\t({})", &entry.id[..8.min(entry.id.len())], content);
            }
        }

        Ok(())
    }
}

fn generate_entry_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let random = RandomState::new().build_hasher().finish();
    format!(
        "{:08x}{:04x}",
        (timestamp & 0xFFFFFFFF) as u32,
        (random & 0xFFFF) as u16
    )
}

fn parse_journal_file(content: &str, date: NaiveDate) -> Result<Vec<BulletEntry>> {
    static ENTRY_RE: OnceLock<Regex> = OnceLock::new();
    let entry_re = ENTRY_RE.get_or_init(|| {
        Regex::new(r"^- \[(.)\] (.+?) \{id:([a-f0-9]{8,12})\}$").unwrap()
    });
    let mut entries = Vec::new();

    for line in content.lines() {
        if let Some(caps) = entry_re.captures(line) {
            let marker = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let text = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let id = caps.get(3).map(|m| m.as_str()).unwrap_or("");

            let (bullet_type, task_state) = match marker {
                " " => (BulletType::Task, Some(TaskState::Incomplete)),
                "x" => (BulletType::Task, Some(TaskState::Complete)),
                ">" => (BulletType::Task, Some(TaskState::Migrated)),
                "<" => (BulletType::Task, Some(TaskState::Scheduled)),
                "o" => (BulletType::Event, None),
                "-" => (BulletType::Note, None),
                _ => continue,
            };

            entries.push(BulletEntry {
                id: id.to_string(),
                bullet_type,
                task_state,
                content: text.to_string(),
                created_at: Utc::now(),
                date,
            });
        }
    }

    Ok(entries)
}

fn format_journal_file(entries: &[BulletEntry], period: JournalPeriod, date_key: &str) -> String {
    let mut output = String::new();

    let header = match period {
        JournalPeriod::Daily => format!("# Daily Log - {}\n\n", date_key),
        JournalPeriod::Weekly => format!("# Weekly Log - {}\n\n", date_key),
        JournalPeriod::Monthly => format!("# Monthly Log - {}\n\n", date_key),
    };
    output.push_str(&header);

    let tasks: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Task)
        .collect();
    let events: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Event)
        .collect();
    let notes: Vec<_> = entries
        .iter()
        .filter(|e| e.bullet_type == BulletType::Note)
        .collect();

    if !tasks.is_empty() {
        output.push_str("## Tasks\n");
        for entry in tasks {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    if !events.is_empty() {
        output.push_str("## Events\n");
        for entry in events {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    if !notes.is_empty() {
        output.push_str("## Notes\n");
        for entry in notes {
            output.push_str(&format_entry(entry));
        }
        output.push('\n');
    }

    output
}

fn format_entry(entry: &BulletEntry) -> String {
    let marker = match (entry.bullet_type, entry.task_state) {
        (BulletType::Task, Some(TaskState::Incomplete)) => " ",
        (BulletType::Task, Some(TaskState::Complete)) => "x",
        (BulletType::Task, Some(TaskState::Migrated)) => ">",
        (BulletType::Task, Some(TaskState::Scheduled)) => "<",
        (BulletType::Event, _) => "o",
        (BulletType::Note, _) => "-",
        _ => " ",
    };
    format!("- [{}] {} {{id:{}}}\n", marker, entry.content, entry.id)
}

fn print_entries(entries: &[BulletEntry]) {
    if entries.is_empty() {
        println!("No entries.");
        return;
    }

    for entry in entries {
        let marker = match (entry.bullet_type, entry.task_state) {
            (BulletType::Task, Some(TaskState::Incomplete)) => "[ ]",
            (BulletType::Task, Some(TaskState::Complete)) => "[x]",
            (BulletType::Task, Some(TaskState::Migrated)) => "[>]",
            (BulletType::Task, Some(TaskState::Scheduled)) => "[<]",
            (BulletType::Event, _) => "[o]",
            (BulletType::Note, _) => "[-]",
            _ => "[ ]",
        };
        println!(
            "{} {} ({}) [{}]",
            marker,
            entry.content,
            entry.date.format("%Y-%m-%d"),
            &entry.id[..4.min(entry.id.len())]
        );
    }
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    let today = Utc::now().date_naive();
    match s.to_lowercase().as_str() {
        "today" => Ok(today),
        "yesterday" => Ok(today - chrono::Duration::days(1)),
        "tomorrow" => Ok(today + chrono::Duration::days(1)),
        _ => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .with_context(|| format!("Invalid date format: {}. Use YYYY-MM-DD", s)),
    }
}
