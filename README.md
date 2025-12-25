# Notes

Local notes CLI with lightweight version tracking and a background daemon that snapshots changes.

## Features
- Create/open notes stored under `~/.notes` (or `NOTES_HOME`).
- Auto-versioning with `notes daemon` (started automatically unless disabled).
- List notes and versions, search content, and roll back to prior versions.
- Shell completion scripts for Bash, Zsh, and Fish.

## Build
```sh
cargo build
```

## Usage
```sh
# Create a note
notes new "Project ideas"

# Open a note by title or id
notes open "Project ideas"

# List notes and versions
notes list
notes versions "Project ideas"

# Roll back to a version
notes rollback "Project ideas" --version 2

# Search in latest versions
notes search "keyword"
```

## Environment
- `NOTES_HOME`: override the storage location (default: `~/.notes`).
- `NOTES_DISABLE_DAEMON`: disable auto-start of the daemon.

## Development
```sh
cargo test
cargo fmt -- --check
cargo clippy -- -D warnings
```
