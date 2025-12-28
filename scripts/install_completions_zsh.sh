#!/bin/sh
set -eu

completion_dir="${1:-"$HOME/.zsh/completions"}"
rc_file="${2:-"$HOME/.zshrc"}"
completion_file="$completion_dir/_notes"

mkdir -p "$completion_dir"

if command -v notes >/dev/null 2>&1; then
  notes completions zsh > "$completion_file"
elif [ -f "Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
  cargo run --quiet -- completions zsh > "$completion_file"
else
  echo "Error: unable to generate completions; install notes or run from repo with cargo." >&2
  exit 1
fi

if [ -f "$rc_file" ]; then
  if ! grep -q "$completion_dir" "$rc_file"; then
    {
      printf '\n# notes completions\n'
      printf 'fpath+=("%s")\n' "$completion_dir"
      printf 'autoload -U compinit && compinit\n'
    } >> "$rc_file"
  fi
else
  {
    printf '# notes completions\n'
    printf 'fpath+=("%s")\n' "$completion_dir"
    printf 'autoload -U compinit && compinit\n'
  } > "$rc_file"
fi

echo "Installed completions to $completion_file"
echo "Reload your shell: source \"$rc_file\""
