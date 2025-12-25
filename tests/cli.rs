use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn setup_home() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn notes_cmd(home: &TempDir) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("notes"));
    cmd.env("NOTES_HOME", home.path());
    cmd.env("NOTES_DISABLE_DAEMON", "1");
    cmd
}

fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn new_creates_working_file_and_index() {
    let home = setup_home();
    let output = notes_cmd(&home)
        .arg("new")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let path = String::from_utf8_lossy(&output).trim().to_string();

    let working = Path::new(&path);
    assert!(working.exists(), "working file should exist");
    let index = home.path().join("index.json");
    assert!(index.exists(), "index.json should exist");

    let versions_dir = home.path().join("versions");
    assert!(versions_dir.exists(), "versions directory should exist");
}

#[test]
fn open_snapshots_changes_and_updates_versions() {
    let home = setup_home();
    let output = notes_cmd(&home)
        .args(["new", "Daily"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let path = String::from_utf8_lossy(&output).trim().to_string();
    let working = Path::new(&path);

    fs::write(working, "first update").expect("write working file");
    notes_cmd(&home).args(["open", "Daily"]).assert().success();

    let versions_dir = home.path().join("versions");
    let slug = "daily";
    let note_versions = versions_dir.join(slug);
    let entries = fs::read_dir(&note_versions)
        .expect("read versions")
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    assert!(
        entries.len() >= 2,
        "expected at least 2 versions after update"
    );
}

#[test]
fn list_versions_outputs_history() {
    let home = setup_home();
    notes_cmd(&home).args(["new", "Meeting"]).assert().success();
    notes_cmd(&home)
        .args(["versions", "Meeting"])
        .assert()
        .success()
        .stdout(contains("Versions for"));
}

#[test]
fn rollback_creates_new_version_and_updates_working_copy() {
    let home = setup_home();
    let output = notes_cmd(&home)
        .args(["new", "Retro"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let path = String::from_utf8_lossy(&output).trim().to_string();
    let working = Path::new(&path);

    fs::write(working, "v2").expect("write working file");
    notes_cmd(&home).args(["open", "Retro"]).assert().success();
    let output = notes_cmd(&home)
        .args(["rollback", "Retro", "--version", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rollback_path = String::from_utf8_lossy(&output).trim().to_string();
    let rollback_content = read_to_string(Path::new(&rollback_path));
    assert!(
        rollback_content.is_empty(),
        "rollback should restore version 1 content"
    );
}

#[test]
fn search_finds_notes_by_content() {
    let home = setup_home();
    let output = notes_cmd(&home)
        .args(["new", "Ideas"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let path = String::from_utf8_lossy(&output).trim().to_string();
    let working = Path::new(&path);

    fs::write(working, "alpha bravo").expect("write working file");
    notes_cmd(&home).args(["open", "Ideas"]).assert().success();

    notes_cmd(&home)
        .args(["search", "bravo"])
        .assert()
        .success()
        .stdout(contains("Ideas"));
}
