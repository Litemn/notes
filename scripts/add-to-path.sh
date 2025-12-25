#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
release_dir="$repo_root/target/release"
binary="$release_dir/notes"

if [[ ! -x "$binary" ]]; then
    echo "Building release binary..."
    (cd "$repo_root" && cargo build --release)
fi

profile=""
if [[ -n "${NOTES_PROFILE:-}" ]]; then
    profile="$NOTES_PROFILE"
else
    shell_name="$(basename "${SHELL:-}")"
    case "$shell_name" in
        zsh)
            profile="$HOME/.zshrc"
            ;;
        bash)
            profile="$HOME/.bashrc"
            ;;
        *)
            profile="$HOME/.profile"
            ;;
    esac
fi

mkdir -p "$(dirname "$profile")"
touch "$profile"

path_line="export PATH=\"$release_dir:\$PATH\""
marker="# notes-cli"

if command -v rg >/dev/null 2>&1; then
    if rg -qF "$release_dir" "$profile"; then
        echo "PATH already contains $release_dir in $profile"
        exit 0
    fi
else
    if grep -qF "$release_dir" "$profile"; then
        echo "PATH already contains $release_dir in $profile"
        exit 0
    fi
fi

{
    echo ""
    echo "$marker"
    echo "$path_line"
} >> "$profile"

echo "Added $release_dir to PATH in $profile"
echo "Run: source $profile"
