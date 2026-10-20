# AGENTS.md: guide for AI coding agents

This file is for AI agents working on this repo. Humans should start with README.md.

## What this is

A zsh tool. Sourcing `ai.zsh` defines the `ai`, `cla` and `cla-team` shell functions. `ai` shows an fzf picker of coding-agent CLIs with plan usage, then starts the chosen one in the current folder. There is no build step, no package manager and no runtime other than zsh, fzf and jq.

## Layout

| Path | Contents |
| --- | --- |
| `ai.zsh` | Entry point: `ai` and its subcommands, the providers file, picker rows, `_ai_exec` (launch). It sources `trust.zsh` and `plugins/*.zsh` at the end. |
| `trust.zsh` | Shared folder trust. One `_has` / `_add` pair per agent, listed in `AI_TRUST`. |
| `plugins/<name>.zsh` | Usage rows or a custom launch for the provider named `<name>`. `claude.zsh` also defines `cla` / `cla-team` account management. |
| `scripts/statusline-usage.sh` | Claude Code status line command that saves `rate_limits` to `$CLAUDE_CONFIG_DIR/usage/usage.json`. |
| `tests/check.zsh` | Self-check of every feature against a fake `$HOME`. |
| `docs/screenshot.png` | README image, rendered from demo data. |

## Contracts to keep

- **Picker row:** `<provider> TAB <id> TAB <shown text>`, printed with `_ai_row`. fzf shows field 3. On selection, `ai` calls `_ai_<provider>_run <id> [args]` if it exists, otherwise `_ai_exec "<saved command>"`.
- **Plugin hooks:** `_ai_<name>_rows` (optional, prints rows) and `_ai_<name>_run <id> [args]` (optional). `<name>` must match a provider name in the providers file.
- **Providers file:** `~/.config/ai-launcher/providers` (override with `$AI_PROVIDERS`), with lines `name<TAB>command<TAB>label`. Only `_ai_put` writes it, and it keeps line order.
- **Launch commands:** a saved string that `_ai_exec` splits with zsh `(z)` and `(Q)`, so quoted arguments work. Never `eval` it.
- **Trust hooks:** `_ai_trust_<name>_has <dir>` and `_ai_trust_<name>_add <dir>`. `<dir>` is an absolute path with symlinks resolved (`${dir:A}`), because agents store `getcwd()`.

## Rules

- **Never lose user config.** Edit other tools' JSON only through `_ai_json_edit`. It writes to a temp file, checks the result is non-empty, then writes back in place to keep permissions. Change only the one key you need.
- **Never widen trust beyond the exact folder,** and never override an explicit untrusted choice (`DO_NOT_TRUST`, `trust_level = "untrusted"`).
- **No network calls.** Usage must come from local files the agents already write.
- **Portable.** It must run on macOS and Linux/WSL. Guard macOS-only commands (`caffeinate`, `security`, `osascript`) with `(( $+commands[x] ))`. Use `date -r` with a `date -d @` fallback.
- **zsh only.** Don't add Python or Node. Keep functions small, and prefix internal ones with `_ai_` or `_cla_`.
- **Rows run in parallel** (`&` in `_ai_rows`). Row functions must not prompt, and must finish quickly even when files are missing.

## Verify a change

```bash
zsh tests/check.zsh              # must print "all passed"
zsh -n ai.zsh trust.zsh plugins/*.zsh
```

Add a check to `tests/check.zsh` for any new behaviour. Tests must use the fake `$HOME` the script sets up, and never real config files. To see the picker yourself, source `ai.zsh` in an interactive zsh and run `ai`. Esc exits without starting anything.

## Regenerate the screenshot

Run `ai` in tmux against fake demo accounts (set `CLA_ROOT`, `CODEX_HOME` and `AI_PROVIDERS` to temp folders), then capture it with `tmux capture-pane -e -p`. Convert the ANSI output to HTML and screenshot it with headless Chromium. Use only fake emails such as `alice@example.com`.
