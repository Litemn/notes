# Notes App: How It Works

## Overview
Notes is a local, file-backed notes database with version control. Every change becomes a new version. You can list notes, inspect version history, roll back to prior versions, and search content.

## Storage Layout
All data lives in `~/.notes` (or `NOTES_HOME` if set).

- `~/.notes/index.json` — metadata for notes and versions.
- `~/.notes/versions/<id>/<NNNNNNN>.md` — immutable version files.
- `~/.notes/files/<id>.md` — current working copy for each note.

The working file is what you edit in your editor. Versions are append-only snapshots.

## Creating a Note
Run:
```bash
notes new
```
or:
```bash
notes new "Title"
```
The command prints the path to the working file (for example, `~/.notes/files/title.md`).

## Opening a Note
Run:
```bash
notes open "Title"
```
This returns the working file path. If the working file has changes compared to the latest version, a new version is created first.

## Versioning Rules
- Each change is stored as a new version file in `~/.notes/versions/`.
- Versions are sequential and never modified.
- The working file always reflects the latest version.

## Listing Notes and Versions
List all notes:
```bash
notes list
```
List versions for a note:
```bash
notes versions "Title"
```

## Rollback
Create a new version from a previous one:
```bash
notes rollback "Title" --version 3
```
If you omit `--version`, it rolls back to the previous version.

## Search
Search the latest versions by text:
```bash
notes search "query"
```

Notes is a database of versioned files; you can use any editor to modify working copies.

## Background Sync Daemon
The first time you run `notes`, it starts a background daemon that watches `~/.notes/files/`.
When a file changes, the daemon creates a new version after a short cooldown.

- Log file: `~/.notes/daemon.log`
- Disable auto-start (for scripts/tests): set `NOTES_DISABLE_DAEMON=1`
