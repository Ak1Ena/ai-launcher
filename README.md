# ai-launcher

One command for all your AI coding agents. Type `ai` to see every agent and account with how much of its plan limit is used, then pick one to start it in the current folder.

![ai picker showing Claude accounts and Codex with usage bars, plus Gemini and OpenCode](docs/screenshot.png)

- **Every agent in one list.** Claude Code (several accounts), Codex, Gemini, Antigravity, Kimi, OpenCode, Qwen, or any CLI you add.
- **Usage at a glance.** Shows 5-hour and weekly limits with reset times. Bars turn yellow at 60% and red at 90%.
- **Your own launch commands.** Add, edit or delete agents and their commands, from the command line or inside the picker.
- **Trust a folder once.** If any agent trusts the folder, every agent you start through `ai` trusts it too, so they stop asking.
- **No API calls.** Usage comes from files the agents already write on your machine.

Works on macOS, Linux and Windows (PowerShell, CMD or WSL). Nothing extra to install: it's plain bash.

## TUI (preview)

A terminal UI that shows every account in a sidebar with its priority, on/off state and usage. Each CLI runs inside a tab on the right. When failover triggers, the session switches to the next account in the same tab, after a 5 second notice (press enter to switch now). When a newer version is on npm, a notice appears in the title bar (checked at most once a day in the background; set `AI_LAUNCHER_NO_UPDATE_CHECK=1` to disable).

Install:

```bash
npm i -g ai-launcher            # once published
cd tui && cargo build --release # or build it yourself (binary: tui/target/release/ai-launcher)
```

Run `ai-launcher [claude args]`. The first CLI you open gets those args.

It reads and writes the same config as the bash `ai` (providers file, failover order and flag), so you can use both.

| Key (sidebar) | Does |
| --- | --- |
| `up` `down` / `k` `j` | move selection |
| `enter` | open the selected account in a tab |
| `a` | add an account |
| `e` | edit the selected account |
| `d` | delete the selected account |
| `shift-up` `shift-down` / `K` `J` | change the order (top is tried first) |
| `space` | leave an account out / back in |
| `f` | failover on / off |
| `r` | clear the account's limit |
| `x` | close tab |
| `1`-`9` | go to tab n |
| `tab` | next tab |
| `q` / `ctrl-c` | quit |

In the add form, a Claude account opens its login in a tab. A Codex or Gemini login gets its own login folder when another one of that CLI exists. Other CLI runs any command you type.

Delete asks first. A Claude account folder moves to `<accounts folder>/.trash`, so its login and history are kept.

| Key (anywhere) | Does |
| --- | --- |
| `alt-1`-`alt-9` | go to tab n, also while typing in a CLI |
| `alt-←` `alt-→` | previous / next tab |
| `ctrl-\` | move between the list and the running CLI |

On macOS, alt (⌥) keys need "Option as Meta" turned on in the terminal settings (Terminal: Profiles > Keyboard; iTerm2: Profiles > Keys).

| Mouse | Does |
| --- | --- |
| click a tab | go to it |
| click a tab's `✕` | close it |
| click an account | select it; click it again to open it |
| `+ Add` / `✎ Edit` / `✕ Delete` | add, edit or delete the selected account |
| Failover switch (title bar) | failover on / off |
| wheel | scroll a CLI's output (unless the CLI uses the mouse itself) |

In a tab, every key goes to the CLI except these:

| Key (in a tab) | Does |
| --- | --- |
| `enter` | on a failover notice: switch now; on an ended tab: restart it |
| `o` | on a failover notice: go to the list to pick another account |
| `q` / `esc` | on a failover notice: stay on the ended session |
| `x` | on an ended tab: close it |

## Install

Nothing to install first. It's plain bash, which is built into macOS and Linux, and comes with Git for Windows (Claude Code on Windows needs Git anyway). It works from any shell.

**macOS / Linux / WSL**

```bash
git clone https://github.com/Ak1Ena/ai-launcher ~/code/ai-launcher
echo 'export PATH="$HOME/code/ai-launcher/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc
```

**Windows (PowerShell)**

```powershell
git clone https://github.com/Ak1Ena/ai-launcher $HOME\code\ai-launcher
$p = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', "$p;$HOME\code\ai-launcher\bin", 'User')
```

`ai.cmd`, `cla.cmd` and `cla-team.cmd` run the same script with Git Bash, so `ai` works from PowerShell, CMD or Git Bash. If Git is installed somewhere unusual, set `CLAUDE_CODE_GIT_BASH_PATH` to its `bash.exe`, the same variable Claude Code uses.

Open a new terminal and type `ai`.

**Optional extras.** Each one turns on more features. Without them, everything else still works:

| Tool | Adds | Built into |
| --- | --- | --- |
| [fzf](https://github.com/junegunn/fzf) | Arrow-key picker with search. Without it you get a numbered menu. | — |
| [jq](https://jqlang.org) | Usage bars, shared folder trust and failover | macOS 15+ |
| tmux | Claude team mode (`ctrl-t`, `cla-team`). Not on native Windows. | — |

```bash
brew install fzf tmux            # macOS (jq is built in on macOS 15+, else: brew install jq)
sudo apt install fzf jq tmux     # Linux / WSL
winget install junegunn.fzf jqlang.jq   # Windows (no tmux, so no team mode outside WSL)
```

**Windows notes:**
- Folder-trust sharing between different agents is off on native Windows for now, because each agent's Windows path format still has to be verified. Sharing trust between Claude accounts still works.
- `cla add` copies your settings into a new account instead of linking them, because Git Bash can't make links by default.

### Show Claude usage

Claude Code passes usage to its status line. To save it, point your status line at the bundled script in `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "bash ~/code/ai-launcher/scripts/statusline-usage.sh" }
```

If you already have a status line command, add it after the script. The script passes the same input on to it:

```json
"statusLine": { "type": "command", "command": "bash ~/code/ai-launcher/scripts/statusline-usage.sh my-statusline --flag" }
```

Usage shows up after your next Claude reply. It is only reported on Pro and Max plans.

## Use it

```bash
ai                 # pick an agent and start it here
ai --resume        # extra arguments go to the agent
```

| With fzf | Without fzf | What it does |
| --- | --- | --- |
| `enter` | `2` | Start the selected agent |
| `ctrl-t` | `t 2` | Start Claude in team mode (teammates in tmux split panes) |
| `ctrl-a` | `a` | Add an agent |
| `ctrl-e` | `e 2` | Edit the selected agent |
| `ctrl-x` | `x 2` | Delete the selected agent |
| `ctrl-f` | `f` | Turn failover on or off |
| `ctrl-o` | `o` | Open the failover priority screen |

### Manage agents

| Command | What it does |
| --- | --- |
| `ai ls` | List agents with their launch command and label |
| `ai add [name] [command] [label]` | Add an agent. It asks for anything you leave out. |
| `ai edit <name> [command] [label]` | Change the command or label. It asks, showing the current value. Press Enter to keep it. |
| `ai rm <name>` | Delete an agent, after asking you to confirm. `ai add <name>` brings it back. |

```bash
ai add aider "aider --model sonnet" "Aider"
ai edit claude "claude --model opus"
```

Agents are saved in `~/.config/ai-launcher/providers`, one per line as `name<TAB>command<TAB>label`. On first run, the file is filled with the agents you have installed. A command runs like a shell alias, so quotes and `VAR=value` prefixes work.

### Several Claude accounts

| Command | What it does |
| --- | --- |
| `cla add work` | Add an account and log in. Settings, skills and plugins are shared with `~/.claude`. |
| `cla` / `cla-team` | Pick a Claude account, then start it normally or in team mode |
| `cla work [team] [args]` | Start a specific account |
| `cla ls`, `cla rm work`, `cla mv work job` | List, remove or rename accounts |

Accounts live in `~/.claude-accounts/<name>`. Set `CLA_ROOT` to use another folder.

### Several Codex or Gemini accounts

Add another agent that runs the same CLI. The first one uses your normal login; every one after it gets its own:

```bash
ai add personal codex "Codex personal"   # codex is already listed, so this is a separate login
```

Start it from `ai` and log in. Each one gets its own row with its own usage bars (Codex). Logins live in `~/.ai-accounts/<cli>/<name>`, which the CLI gets as `CODEX_HOME` or `GEMINI_CLI_HOME`. Set `AI_ACCOUNTS` to use another folder. A new login starts with fresh settings: nothing is copied from the main one. `ai rm` keeps the folder, so adding the same name again gets the same login back.

Antigravity (`agy`) can't have several accounts yet: it always reads `~/.gemini/antigravity-cli` and has no setting to move it.

### Failover

When a Claude account hits its usage limit, the same session resumes on the next account in your priority order. Codex has no limit hook, so it only gets a written handoff of the session, and the session ends there: accounts below Codex in the order are not tried. New Codex accounts start at the bottom. Failover is off until you turn it on.

```bash
ai failover on     # or ctrl-f in the picker
ai failover off
ai failover ls     # members in order, with any limit still running
```

Set the priority with `ai failover` (or `ctrl-o` in the picker). On that screen:

| With fzf | Without fzf | What it does |
| --- | --- | --- |
| `ctrl-u` / `ctrl-d` | `u 2` / `d 2` | Move the member up / down |
| `ctrl-x` | `x 2` | Turn the member on / off |
| `ctrl-r` | `r 2` | Clear its limit |
| `ctrl-f` | `f` | Turn failover on / off |

```bash
ai failover order claude/work claude/personal codex   # put these first, in this order
ai failover clear claude/work                         # forget a limit (no name = all)
```

| Variable | Default | What it does |
| --- | --- | --- |
| `AI_FO_COOLDOWN` | `18000` | Seconds an account is skipped after it hits its limit |
| `AI_FO_STATE` | `${XDG_STATE_HOME:-~/.local/state}/ai-launcher/failover` | Where limit marks are kept |
| `AI_FO_PROMPT` | "...Continue the work exactly where you left off." | First message sent to the new account |

Needs jq. Not used in team mode. Limits: the cooldown is fixed, not the real reset time (use `ctrl-r` if the limit ended sooner). Codex can't trigger a failover, so put it last. Not available on native Windows (Git Bash yet); use WSL.

## How it works

**Usage.** Nothing is fetched from the internet:
- **Claude:** reads `<account>/usage/usage.json`, written by `scripts/statusline-usage.sh`. The numbers are from that account's last session. Rows older than 10 minutes show their age, and a limit whose reset time has passed shows 0%.
- **Codex:** reads the rate limits saved in the newest `sessions` log of `~/.codex`, or of that agent's own login folder.
- **Other agents** don't save usage, so they show "no usage data".

**Folder trust.** When you start an agent through `ai` or `cla`, it checks whether the folder is trusted by any of Claude (main config and every account), Codex, Gemini, Qwen or Antigravity. If one trusts it, the folder is marked trusted in the others too.
- Only that exact folder is shared. Parent folders don't count.
- A folder you marked untrusted in an agent stays untrusted there.

**Staying awake.** On macOS, agents run under `caffeinate -i` so the machine doesn't sleep mid-task.

## Add usage bars for another agent

An agent with no plugin gets a plain row. To show usage, create `plugins/<name>.sh`, where `<name>` is the agent's name in `ai ls`:

```bash
_ai_mytool_rows() {                      # print one picker row per account
  _ai_row mytool <id> <shown-name> "<label>" "$(_ai_bar 5h <percent> '<reset time>')"
}
_ai_mytool_run() { shift; _ai_exec "$(_ai_cmd mytool mytool)" "$@"; }   # optional; $1 = row id
```

To share folder trust with another agent, add its name to `AI_TRUST` and define `_ai_trust_<name>_has` and `_ai_trust_<name>_add` in `lib/trust.sh`.

## Test

```bash
bash tests/check.sh
/bin/bash tests/check.sh     # on macOS: also check with the built-in bash 3.2
```

This runs every feature against a throwaway home folder and never touches your real configs.

## Releases

CI runs on branches and PRs. Every merge to main builds all platforms and creates or updates a draft release `v<version>`, where the version is in `tui/Cargo.toml`. Publishing the draft triggers the npm publish (needs the `NPM_TOKEN` secret). Bump the version in `tui/Cargo.toml` before the next release.

## License

MIT
