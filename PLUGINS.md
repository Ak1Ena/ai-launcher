# Writing a plugin for ai-launcher

This guide is for developers and AI agents who build a plugin. It is short on purpose. The README "Plugins" section has the user side (install, update, TUI keys).

A plugin adds your own entries to `ai` (the bash picker and the TUI), with no change to this repo. It is a folder with an executable file called `plugin`, in any language. Plugin entries are never part of failover.

## 1. Quick start

```bash
ai plugin new demo                       # copies templates/plugin/ to ~/.config/ai-launcher/plugins/demo/
$EDITOR ~/.config/ai-launcher/plugins/demo/plugin
ai                                       # a "Demo" section shows up
```

Develop in your own folder and install a copy when it works:

```bash
ai plugin add ~/code/my-plugin           # local folder (copied in)
ai plugin rm my-plugin                   # remove it again (add never overwrites)
```

Publish: put `plugin` (and optionally `setup`, `about`) at the top of a git repo. Others install it with:

```bash
ai plugin add owner/repo                 # becomes https://github.com/owner/repo.git
ai plugin add https://example.com/x/y.git [name]
```

Git sources are HTTPS only. The plugin name is the repo name unless you pass `[name]`. The name is letters, digits, `_` or `-`.

## 2. Folder layout

Plugins live in `~/.config/ai-launcher/plugins/<name>/` (set `AI_PLUGINS` to use another folder).

| File | Needed | What it is |
| --- | --- | --- |
| `plugin` | yes | Executable (on Windows it runs as `bash plugin`). Answers the commands in section 3. |
| `setup` | no | Runs once, after the user reads and approves it. Section 5. Must be a regular file, never a link. |
| `about` | no | First line is a short description (60 characters, control characters dropped). Shown in `ai plugin ls`, the available list and the TUI Plugins screen. |
| `config` | by convention | Your own settings. Keep config and cache inside the plugin folder (`$AI_PLUGIN_DIR`), mode 600 for anything secret. `ai plugin setup` copies `config` to `config.bak` before it re-runs setup. |
| `.off` | made by `ai plugin off` | Empty file. The plugin is never run, and its section shows `<name> · off`. Do not ship it (`add` deletes it). |
| `.source` | made by `add` | The git URL it came from, used by `ai plugin update`. Do not ship it. |

## 3. Protocol

`ai` runs `plugin <command>` in the user's current folder, with no tty and `AI_PLUGIN_DIR` set to the plugin folder. Fields are separated by a TAB.

| Command | Output | Limit |
| --- | --- | --- |
| `rows` | First line `section<TAB><title>`. Then one line per entry: `<id><TAB><label><TAB><status text>`. A line `  <TAB><text>` (two spaces, a tab) is a detail line under the entry above it. | 2 s. In the TUI a plugin's first `rows` / `actions` / `menu` in a session gets 10 s. Failure or timeout shows `<name>: plugin failed`. |
| `actions` (optional) | One line per action: `<action><TAB><label>`. | 2 s (first call in the TUI: 10 s) |
| `menu` (optional, TUI only) | Tabs for the TUI `a` window. Section 4. | 2 s (first call in the TUI: 10 s) |
| `do <action>` | Runs one action. stdin is closed. stdout and stderr are shown to the user. `rows` is refreshed afterwards. | 30 s, then it is killed |
| `launch <id> [args...]` | Runs the session in the foreground with the terminal (bash: `exec`, TUI: its own tab). | none |

Rules for the output:

- An entry `<id>` and an action id are letters, digits, `.`, `_` or `-` (`[A-Za-z0-9._-]+`). A row with another id is skipped.
- A row with no TAB is skipped. Extra TAB fields after the label are joined into the status.
- Status and detail text is plain text. The bash picker shows ANSI colour, the TUI strips it. Control characters are removed.
- Other files refer to an entry as `plugin:<name>/<id>` (for example the default entry). An id that no longer exists is ignored.
- If `rows` prints no valid entry, the section shows `no entries`.
- In bash `ai`, a launch shares folder trust like any other (only with `ai trust on`). The TUI has no shared trust.

A complete small plugin:

```bash
#!/usr/bin/env bash
case ${1:-} in
  rows)
    printf 'section\tDemo\n'
    printf 'hello\tHello\tready\n'
    printf '  \tfolder: %s\n' "$AI_PLUGIN_DIR"
    ;;
  actions) printf 'ping\tPing\n' ;;
  do)
    case ${2:-} in
      ping) echo pong ;;
      *) echo "unknown action: ${2:-}"; exit 1 ;;
    esac
    ;;
  launch) exec "${SHELL:-bash}" ;;   # $2 is the id, the rest are extra args
  *) exit 1 ;;
esac
```

## 4. Menu (TUI only)

With `menu`, `a` on a plugin row (or Enter in the Plugins screen) opens one window with a tab per `tab` line. Without `menu`, `a` shows the `actions` form. Bash `ai` ignores `menu`. `ai plugin do <name> <id>` runs the same ids.

All lines are TAB separated. Anything else is skipped, and a tab left empty is dropped.

```text
tab<TAB>list|radio|checklist<TAB><title>[<TAB>order|weight]
item<TAB><id><TAB><label>[<TAB>on]
row<TAB><key><TAB><label><TAB>1|0<TAB><n|-><TAB><note><TAB><on-id><TAB><off-id><TAB><dec-id><TAB><inc-id>
```

- `list`: its `item` lines. Enter runs `do <id>`.
- `radio`: the same, and `on` marks the current choice.
- `checklist`: its `row` lines. The mode is `order` (default) or `weight`.
  - `1` = ticked (in use), `0` = not.
  - `<n>` is the position (`order`) or the weight 0..99 (`weight`). `-` means not placed (for example not linked yet): no number, and it never moves.
  - `on-id` / `off-id` run when the user ticks / unticks the row. `dec-id` / `inc-id` move it up / down (`order`) or change the weight by -1 / +1 (`weight`). An id of `-` means that change is not offered.
- Every id and `key` is an action id (`[A-Za-z0-9._-]+`).

Example:

```bash
menu)
  printf 'tab\tlist\tActions\n'
  printf 'item\tping\tPing\n'
  printf 'tab\tradio\tMode\n'
  printf 'item\tmode-a\tmode A%s\n' "$([ "$mode" = a ] && printf '\ton')"
  printf 'item\tmode-b\tmode B%s\n' "$([ "$mode" = b ] && printf '\ton')"
  printf 'tab\tchecklist\tAccounts\torder\n'
  #       row  key  label  ticked n  note on-id   off-id   dec-id  inc-id
  printf 'row\tw1\tWork\t1\t1\t\ton.w1\toff.w1\tup.w1\tdown.w1\n'
  printf 'row\tp1\tHome\t0\t-\t\ton.p1\t-\t-\t-\n'
  ;;
```

How the TUI applies a checklist:

- Keys: left/right (or Tab) switch tabs, up/down move, space ticks, `shift-up/down` (or `K` / `J`) reorders an `order` list, `+` / `-` changes a weight.
- Nothing runs until Enter. Esc closes with no change.
- On Enter the TUI compares the list with the one it opened with and runs `do` once per step, in this order:
  1. ticks of rows that are not placed (`on-id`, for example a link),
  2. other ticks (`on-id` / `off-id`),
  3. moves (a row's `dec-id` once for each place it rises, which reaches any new order),
  4. weights (`inc-id` or `dec-id` once per step).
- A failure does not stop the rest. Then the plugin reloads and one line says `applied N changes` or `K of N changes failed: <id> failed: <why>; ...`.
- The window runs the ids of the menu it opened with, never a list refreshed meanwhile. So every `do` id must keep working for as long as the window is open, and each step must be safe to run on its own.

The bundled `plugins-extra/cliproxy/plugin` (`menu)` branch) is a worked example with a `list` and a `weight` checklist.

## 5. Setup script

`setup` is for work that must happen once: install a tool, make keys, log in.

- On `ai plugin add`, the plugin is copied into a hidden staging folder (`.<name>.install`, never listed, never run). The whole `setup` is shown with line numbers. Only after the user approves (`y`) it runs. The reviewed copy is the one that runs.
- It runs once, with a tty (the user's terminal), in the plugin folder, with `AI_PLUGIN_DIR` set. No timeout.
- Exit 0 means installed. Anything else means failed: a new install is removed (nothing is left behind), an existing plugin is kept.
- It must be safe to run again (`ai plugin setup <name>`, or `s` in the TUI Plugins screen). Check before you change anything. Never overwrite user config.
- `ai plugin update` never runs `setup`. It only says `setup changed: run setup again to apply`.
- It is shown in full, so keep it short and readable, with no hidden or control characters (they are shown as `^X` in bash and `<U+XXXX>` in the TUI).
- The network may be used inside `setup` (for example a release download with a checksum check) and inside your own actions. Nowhere else.

Pattern (from the template):

```bash
#!/usr/bin/env bash
set -e
cd "${AI_PLUGIN_DIR:-.}"
if [ -e config ]; then
  echo "config exists: leaving it alone"
else
  ( umask 077; printf '# settings\n' > config )
  chmod 600 config
fi
```

## 6. Rules

- **Bash 3.2.** If you write bash, it must run on the bash that ships with macOS. No `declare -A`, `mapfile`, `${var,,}`, `$EPOCHSECONDS`, `&>>`, negative array indexes. Use `date +%s`. Git Bash on Windows must work too, so avoid GNU-only flags.
- **Nothing to install.** Do not require tools beyond bash and the usual system ones. If you use `jq`, `fzf` or another tool, check it first (`command -v jq`) and degrade gracefully when it is missing.
- **Never print secrets.** No keys, tokens or URLs with credentials, in `rows`, `actions`, `menu`, `do` output or errors. Keep secrets in files with mode 600 under `$AI_PLUGIN_DIR`.
- **Fast rows.** `rows`, `actions` and `menu` have a 2 s limit. Cache anything slow in a file under `$AI_PLUGIN_DIR` and read the cache. No network in `rows` unless the plugin's purpose needs it, and then cache the answer.
- **No prompts** outside `setup`. There is no tty for `rows`, `actions`, `menu` and `do`. `launch` is the only command that owns the terminal.
- **Keep state in the plugin folder.** Do not write to other tools' config. Never lose user config.
- **Plain text only** in output. Same id rules as section 3.
- **Plugin code runs with the user's permissions.** Do only what the plugin says it does.

## 7. Testing your plugin

By hand, from your plugin folder:

```bash
export AI_PLUGIN_DIR=$PWD
./plugin rows | cat -A | head       # check the TABs and the first line
./plugin actions
./plugin menu
./plugin do ping </dev/null         # the real run has no stdin
bash setup; bash setup              # run setup twice: the second run must change nothing
time ./plugin rows                  # must be well under 2 s
```

Through `ai` (same limits and env as for real):

```bash
ai plugin add ./                    # or ai plugin new / edit in place under ~/.config/ai-launcher/plugins
ai plugin ls                        # shows on/off and the section title
ai plugin do <name> <action>        # runs `plugin do <action>` with the 30 s limit
ai                                  # the section and the entries appear
```

In the TUI, press `a` on your plugin row to try the menu, or open the Plugins screen with `P`.

This repo tests a plugin in a fake environment, see `tests/cliproxy.sh`: a temporary `$HOME`, a fake server and fake commands first on `PATH`, and never real config. Use `AI_PLUGINS` to point at a temporary plugins folder, `AI_YES=1` to approve the setup review without asking (tests only), and `AI_PLUGIN_DO_TIMEOUT` to shorten the `do` limit.

## 8. Checklist before publishing

- [ ] `plugin` is executable (`chmod +x plugin`) and has a shebang.
- [ ] `rows` starts with `section<TAB><title>`, uses TABs, and ids match `[A-Za-z0-9._-]+`.
- [ ] `rows`, `actions` and `menu` finish well under 2 s, with no tty and no prompts.
- [ ] `do` finishes in under 30 s, and prints a clear message on failure and exits non-zero.
- [ ] Every menu id is also handled by `do`.
- [ ] No secret is ever printed. Secret files are mode 600.
- [ ] Works on bash 3.2 (`/bin/bash` on a Mac) and has no required extra tools.
- [ ] `setup` (if any) is a regular file, short, readable, safe to run twice, and never overwrites user config.
- [ ] `about` is one line of 60 characters or less.
- [ ] No `.off` or `.source` file in the repo.
- [ ] Installed with `ai plugin add` from a clean folder, then removed with `ai plugin rm`, with no leftovers.
