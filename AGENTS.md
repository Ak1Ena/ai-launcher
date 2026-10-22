# AGENTS.md: guide for AI coding agents

This file is for AI agents working on this repo. Humans should start with README.md.

## What this is

A bash command-line tool. `bin/ai` shows a picker of coding-agent CLIs with plan usage, then starts the chosen one in the current folder. `bin/cla` and `bin/cla-team` are tiny scripts that call `bin/ai cla…`. They are not symlinks, because Git on Windows turns symlinks into text files. `bin/*.cmd` start the same scripts with Git Bash from PowerShell or CMD. There is no build step and no package manager.

**Hard requirement: nothing to install.** The tool must run on the bash built into macOS (3.2), Linux and WSL, and on Git Bash on Windows. fzf, jq and tmux are optional, and every feature that needs one must degrade gracefully when it is missing.

## Layout

| Path | Contents |
| --- | --- |
| `bin/ai` | Entry point. Finds the repo by following symlinks, sources `lib/` and `plugins/`, calls `ai_main`. |
| `lib/core.sh` | `ai_main` and its subcommands, the providers file, rows and bars, the picker (fzf or numbered menu), and launching (`_ai_exec`). |
| `lib/trust.sh` | Shared folder trust. One `_has` / `_add` pair per agent, listed in `AI_TRUST`. |
| `lib/failover.sh` | Failover: `ai failover` commands, priority screen, and `_fo_run` (relaunches the session on the next account after a limit). |
| `mods/failover/` | Claude Code mod. Its `classic.StopFailure` hook, on `rate_limit`, writes `$AI_FO_HANDOFF` and ends that claude. |
| `plugins/<name>.sh` | Usage rows or a custom launch for the provider named `<name>`. `claude.sh` also implements `cla` / `cla-team`. |
| `scripts/statusline-usage.sh` | Claude Code status line command that saves `rate_limits` to `$CLAUDE_CONFIG_DIR/usage/usage.json`. |
| `tests/check.sh` | Self-check of every feature against a fake `$HOME`. |
| `docs/screenshot.png` | README image, rendered from demo data. |

## Contracts to keep

- **Picker row:** `<provider> TAB <id> TAB <shown text>`, printed with `_ai_row`. On selection, `ai` calls `_ai_<provider>_run <id> [args]` if it exists, otherwise `_ai_exec "<saved command>"`.
- **`_ai_choose` output:** `<action>\n<row>`, where action is empty (launch), `team`, `add`, `edit`, `delete`, `failover` or `order`. A third arg `keys` is the main picker, and `order` is the failover priority screen, whose actions are empty (back), `up`, `down`, `toggle`, `clear` or `failover`. It returns non-zero on cancel. The fzf and menu versions must behave the same.
- **Failover files:** order file `failover` next to the providers file (`$AI_FO_ORDER`, lines `member<TAB>on|off`), flag file `failover.on` beside it (failover is on only with the flag and jq), and state dir `$AI_FO_STATE` (`limited/<member>` marks by mtime, `handoff.<pid>.json`). Members are `claude/<account>` for each cla account, or the name of a provider that runs claude or codex. Not used in team mode.
- **Plugin hooks:** `_ai_<name>_rows` (optional, prints rows, only called when jq exists) and `_ai_<name>_run <id> [args]` (optional). `<name>` matches a provider name in the providers file.
- **Providers file:** `~/.config/ai-launcher/providers` (override with `$AI_PROVIDERS`), with lines `name<TAB>command<TAB>label`. Only `_ai_put` writes it, and it keeps line order.
- **Launch commands:** the saved string is the user's own config, so it runs through `bash -c` like a shell alias, with extra args passed as `"$@"`.
- **Trust hooks:** `_ai_trust_<name>_has <dir>` and `_ai_trust_<name>_add <dir>`. `<dir>` is the resolved path (`pwd -P`), because agents store `getcwd()`.

## Bash 3.2 rules

macOS ships bash 3.2, so do not use:
- associative arrays (`declare -A`)
- `mapfile` / `readarray`
- `read -i`
- `${var,,}` / `${var^^}`
- `declare -g`
- `$EPOCHSECONDS`
- `&>>`
- negative array indexes

Use `date +%s` for the current time. Use `${arr[@]+"${arr[@]}"}` to expand arrays that may be empty. Run the tests with `/bin/bash` on a Mac.

## Rules

- **Never lose user config.** Edit other tools' JSON only through `_ai_json_edit`. It writes to a temp file, checks the result is non-empty, then writes back in place to keep permissions. Change only the one key you need.
- **Never widen trust beyond the exact folder,** and never override an explicit untrusted choice (`DO_NOT_TRUST`, `trust_level = "untrusted"`).
- **No network calls.** Usage must come from local files the agents already write.
- **Portable.** Guard optional or OS-specific commands with `has <cmd>`: fzf, jq, tmux, `caffeinate`, `security`. For dates use `date -r` with a `date -d @` fallback. Don't use `stat` (its flags differ between macOS and Linux).
- **Rows run in parallel** (background jobs in `_ai_rows`). Row functions must not prompt, and must finish quickly even when files are missing.
- **Windows (Git Bash):**
  - Paths handed to native programs go through `native_path`, which turns them into `C:/...`.
  - Never read `/dev/tty`. Pickers draw on stderr and read from stdin.
  - Keep LF endings in scripts and CRLF in `.cmd` files (see `.gitattributes`).
  - Check for Windows with `is_windows`.
  - Cross-agent trust sharing is off there until each agent's Windows path format is verified.
- **Width:** pad shown text with `pad` (counts characters and ignores color codes), not `printf %-Ns`, which counts bytes.

## Verify a change

```bash
bash tests/check.sh                 # must print "all passed"
/bin/bash tests/check.sh            # macOS: same on bash 3.2
for f in bin/ai lib/*.sh plugins/*.sh; do bash -n "$f"; done
```

Add a check to `tests/check.sh` for any new behaviour. Tests must use the fake `$HOME` the script sets up, and never real config files.

To try the numbered menu, run with fzf hidden: `PATH=$PWD/bin:/usr/bin:/bin ai`. Choosing `q` exits without starting anything.

## Regenerate the screenshot

Run `ai` in tmux against fake demo accounts (set `CLA_ROOT`, `CODEX_HOME` and `AI_PROVIDERS` to temp folders), then capture it with `tmux capture-pane -e -p`. Convert the ANSI output to HTML and screenshot it with headless Chromium. Use only fake emails such as `alice@example.com`.
