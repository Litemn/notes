use crate::cli::CompletionShell;

pub fn print_completions(shell: CompletionShell) {
    let script = match shell {
        CompletionShell::Bash => BASH_COMPLETION,
        CompletionShell::Zsh => ZSH_COMPLETION,
        CompletionShell::Fish => FISH_COMPLETION,
    };
    print!("{script}");
}

const BASH_COMPLETION: &str = r#"_notes_ids() {
  NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null | tr '\n' ' '
}

_notes_bullet_ids() {
  # Output format: "id<TAB>(description)" - we extract just the id for completion
  NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null | cut -f1 | tr '\n' ' '
}

_notes_complete() {
  local cur cmd
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmd="${COMP_WORDS[1]}"

  case "$cmd" in
    open|versions|delete|rollback)
      local has_title=0
      local i=2
      while [[ $i -lt $COMP_CWORD ]]; do
        local word="${COMP_WORDS[i]}"
        if [[ "$word" == "--version" || "$word" == "-v" ]]; then
          ((i+=2))
          continue
        fi
        if [[ "$word" != -* ]]; then
          has_title=1
          break
        fi
        ((i+=1))
      done
      if [[ $has_title -eq 0 && "$cur" != -* ]]; then
        local ids
        ids=$(_notes_ids)
        COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
      fi
      return 0
      ;;
    bullet|b)
      if [[ $COMP_CWORD -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "list pending complete migrate open search interactive" -- "$cur") )
      elif [[ $COMP_CWORD -eq 3 ]]; then
        local subcmd="${COMP_WORDS[2]}"
        if [[ "$subcmd" == "complete" || "$subcmd" == "x" ]]; then
          local ids
          ids=$(_notes_bullet_ids)
          COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
        fi
      fi
      return 0
      ;;
  esac
}

complete -F _notes_complete notes
"#;

const ZSH_COMPLETION: &str = r#"#compdef notes
autoload -U +X bashcompinit && bashcompinit

_notes_ids() {
  NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null | tr '\n' ' '
}

_notes_bullet_ids() {
  # Output format: "id<TAB>(description)" - we extract just the id for completion
  NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null | cut -f1 | tr '\n' ' '
}

_notes_complete() {
  local cur cmd
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmd="${COMP_WORDS[1]}"

  case "$cmd" in
    open|versions|delete|rollback)
      local has_title=0
      local i=2
      while [[ $i -lt $COMP_CWORD ]]; do
        local word="${COMP_WORDS[i]}"
        if [[ "$word" == "--version" || "$word" == "-v" ]]; then
          ((i+=2))
          continue
        fi
        if [[ "$word" != -* ]]; then
          has_title=1
          break
        fi
        ((i+=1))
      done
      if [[ $has_title -eq 0 && "$cur" != -* ]]; then
        local ids
        ids=$(_notes_ids)
        COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
      fi
      return 0
      ;;
    bullet|b)
      if [[ $COMP_CWORD -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "list pending complete migrate open search interactive" -- "$cur") )
      elif [[ $COMP_CWORD -eq 3 ]]; then
        local subcmd="${COMP_WORDS[2]}"
        if [[ "$subcmd" == "complete" || "$subcmd" == "x" ]]; then
          local ids
          ids=$(_notes_bullet_ids)
          COMPREPLY=( $(compgen -W "$ids" -- "$cur") )
        fi
      fi
      return 0
      ;;
  esac
}

complete -F _notes_complete notes
"#;

const FISH_COMPLETION: &str = r#"function __notes_ids
    NOTES_DISABLE_DAEMON=1 notes ids 2>/dev/null
end

function __notes_bullet_ids
    # Output format: "id<TAB>(description)" - Fish handles this natively for descriptions
    NOTES_DISABLE_DAEMON=1 notes bullet ids 2>/dev/null
end

function __notes_needs_id
    set -l cmd (commandline -opc)
    if test (count $cmd) -lt 2
        return 1
    end
    set -l sub $cmd[2]
    switch $sub
        case open versions delete rollback
            set -l i 3
            while test $i -le (count $cmd)
                set -l word $cmd[$i]
                if test "$word" = "--version" -o "$word" = "-v"
                    set i (math $i + 2)
                    continue
                end
                if not string match -qr '^-.*' -- $word
                    return 1
                end
                set i (math $i + 1)
            end
            return 0
    end
    return 1
end

function __notes_bullet_subcommand
    set -l cmd (commandline -opc)
    if test (count $cmd) -eq 2
        if test "$cmd[2]" = "bullet" -o "$cmd[2]" = "b"
            return 0
        end
    end
    return 1
end

function __notes_bullet_needs_id
    set -l cmd (commandline -opc)
    if test (count $cmd) -eq 3
        if test "$cmd[2]" = "bullet" -o "$cmd[2]" = "b"
            if test "$cmd[3]" = "complete" -o "$cmd[3]" = "x"
                return 0
            end
        end
    end
    return 1
end

complete -c notes -n '__notes_needs_id' -a '(__notes_ids)'
complete -c notes -n '__notes_bullet_subcommand' -a 'list pending complete migrate open search interactive'
complete -c notes -n '__notes_bullet_needs_id' -a '(__notes_bullet_ids)'
"#;
