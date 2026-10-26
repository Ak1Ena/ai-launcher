# User plugins, and a cliproxy plugin

Date: 2026-10-26. Status: draft for approval.

## Why

Users want to add their own launch targets to ai-launcher without editing the repo or waiting for a
release. The first one is CLIProxyAPI (reference setup: github.com/DeathSy/dotfiles, `cliproxyapi/`
and `nixpkgs/home-manager/ai/gateway.nix`): a local proxy that holds OAuth logins for Claude, Codex,
Gemini and more, so Claude Code (the harness: tools, skills, hooks) can run GPT / Gemini models and
spread load across every logged-in account. The reference wires it into `~/.claude/settings.json`
for every session; here it must be an opt-in plugin, separate from the account switcher.

## Goals

1. **Plugins for everyone, in both UIs.** A user drops a plugin folder in, and both the Rust TUI
   (npm `ai`) and bash `ai` pick it up.
2. **Plugin owns its section.** Each plugin's entries show in their own section on the TUI home list
   (and as their own group in the bash picker), titled by the plugin, not by core code.
3. **Plugin rows are outside failover.** Core never puts plugin rows in the failover order, WRR, or
   the limited marks. Anything balancing-like is the plugin's (or its backend's) job.
4. **cliproxy plugin, one channel for all accounts.** One entry, "Claude Code · all accounts", that
   starts `claude` against the proxy. The proxy load-balances across every account logged in to it;
   the user picks GPT / Gemini / Claude models inside Claude Code (`/model`, gateway model
   discovery).

## Non-goals

- No plugin marketplace or registry, no auto-download, no background auto-update of plugins.
- The cliproxy plugin does not install CLIProxyAPI, log accounts in to it, or edit its config.
- No change to `~/.claude/settings.json` or any cla account: proxy env is set per session only.
- No per-model rows (the user chose one channel).
- Existing bash `plugins/*.sh` (claude.sh, codex.sh usage rows) stay as they are; they are internal.

## Anti-goals (a failure even if it works)

- A broken, slow or missing plugin breaks or slows the home list / picker.
- Core code grows cliproxy-specific branches.
- A plugin row ends up in failover, or a failover move lands on a plugin row.
- The proxy key ends up in a world-readable file or on screen.

## Constraints (from AGENTS.md)

- Bash side: bash 3.2, Git Bash on Windows; no new required tools.
- TUI and bash share files and formats; new state goes in new files.
- "No network calls" rule: core makes none for plugins. A plugin is user-installed code (same trust
  as a providers-file command), so what it does is on the plugin. Core runs `rows` with a timeout.
- Tests use the fake `$HOME` in `tests/check.sh`; Rust tests via `cargo test`.

## Design

### Plugin layout and protocol

```
~/.config/ai-launcher/plugins/<name>/      (beside the providers file; override $AI_PLUGINS)
  plugin          executable, any language (on Windows: run through bash, so a bash script works)
  ...             anything else the plugin wants (its config, state)
```

`<name>`: `[A-Za-z0-9_-]+`; other folders are skipped.

Core calls, with cwd = the user's current folder and env `AI_PLUGIN_DIR=<the plugin folder>`:

| Call | Output / effect | Limits |
| --- | --- | --- |
| `plugin rows` | Line 1: `section<TAB><title>`. Then one line per entry: `<id><TAB><label><TAB><status text>`. | 2 s timeout, stdout only, no tty. Non-zero exit or timeout: the section shows one dim line `<name>: plugin failed`. |
| `plugin actions` (optional) | Lines `<key><TAB><action><TAB><label>`, e.g. `S<TAB>start<TAB>start proxy`. Shown as key hints on the plugin's rows. | 2 s timeout. Unknown/failed: no actions. |
| `plugin do <action>` | Runs a plugin action (TUI: in a small tab / flash with its output; bash: `ai plugin do <name> <action>`), then `rows` is refreshed. | 30 s timeout. |
| `plugin launch <id> [args…]` | Runs the session in the foreground with the terminal (TUI: in a new tab's PTY; bash: `exec`). | none |

`<id>`: `[A-Za-z0-9._-]+`. Status text is plain text (TUI) or may hold ANSI colour (bash shows it as
is, TUI strips it).

Shared entry id in other files (default entry, tab names): `plugin:<name>/<id>`. The `.default` file
may hold it; a missing plugin/id is ignored as today.

### Management commands (bash `ai`; TUI reads the same folder)

Install works for any plugin, now and later: a plugin is just a folder that follows the protocol,
so nothing in core is tied to a specific plugin.

- `ai plugin ls` — installed plugins (name, source, section title).
- `ai plugin add <source> [name]` — `<source>` is one of:
  - a local folder: copied in;
  - a git URL (`https://…`, `git@…`, `owner/repo` shorthand for GitHub): `git clone --depth 1`
    (needs `git`); the source is saved in `<plugin>/.source` for updates;
  - a bundled name (`cliproxy`): copied from the repo's `plugins-extra/<name>/` (for npm users:
    bundled in the npm package next to the binary).
  It validates that `plugin` exists and is executable, refuses to overwrite an existing plugin, and
  prints what it installed and from where.
- `ai plugin update [name]` — `git pull --ff-only` for git-installed plugins (all when no name).
- `ai plugin off <name>` / `ai plugin on <name>` — stop / start a plugin without removing it.
  Off = an empty marker file `<plugin>/.off`; core then never runs it, and its section shows one
  dim line `<title> · off` (TUI: toggle with a key / click on the section header, like the failover
  pill). New installs start on.
- `ai plugin rm <name>` — asks `[y/N]`, removes the folder (validated name, no symlink following).
- TUI: an "add plugin" form (same `<source>` field) and delete on a plugin section, calling the same
  logic (Rust implementation of the same steps, or shelling out to the bundled bash `ai plugin`;
  builder's call).

Network: install / update are explicit user commands, never run in the background. This is a third
network exception to add to AGENTS.md ("only when the user runs `ai plugin add|update` with a git
source").

### TUI

- Home list order: failover section (unchanged), OTHER CLIS (unchanged), then one section per
  plugin, header = the plugin's `section` title, plugins sorted by name.
- Plugin rows have no priority, no on/off pill, no weight; failover/WRR code skips them.
- Enter opens a tab running `<plugin dir>/plugin launch <id>`; the tab title is the label.
- `rows` runs on a background thread at startup and on refresh, same cadence as usage refresh.

### Bash picker

- After the provider rows, each plugin's rows, with a dim header line per section in the menu and
  fzf (fzf: a non-selectable header row or a prefix tag, builder's call).
- Picker row contract: provider field = `plugin:<name>`, id = `<id>`. Selection runs
  `plugin launch <id> "$@"`.

### The cliproxy plugin (`plugins-extra/cliproxy/`)

- `plugin` (bash script) + `config` (created on first `rows` if missing, mode 600):
  ```
  url=http://127.0.0.1:8317
  key_env=CLIPROXY_API_KEY      # name of the env var holding the client key
  # key=...                     # or the key itself (file stays 600)
  ```
- `rows`: prints `section<TAB>CLIPROXY · all accounts` then one entry
  `all<TAB>Claude Code · all accounts<TAB><status>`. Status: `proxy up` / `proxy not running — start: brew services start cliproxyapi` / `no key: set CLIPROXY_API_KEY`. The up check is one TCP/HTTP
  probe to the configured localhost URL with a 1 s timeout (`curl -s -m 1`, fall back to bash
  `/dev/tcp` when curl is missing).
- `actions`: `start` / `stop` the proxy (`brew services start|stop cliproxyapi` when brew has it,
  else the commands from `config` keys `start_cmd` / `stop_cmd`; no action shown when neither
  exists).
- `launch all [args]`: if the proxy is down, print the same hint and exit 1 (does not start it).
  Otherwise `exec env ANTHROPIC_BASE_URL=$url ANTHROPIC_AUTH_TOKEN=$key
  CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1 claude "$@"`, using the main `~/.claude` config
  (unset any inherited `CLAUDE_CONFIG_DIR`).
- Never prints the key.

## Builder's calls

- Exact module/file split in `tui/src` and `lib/` (e.g. `tui/src/plugins.rs`, `lib/plugins.sh`).
- fzf section rendering detail; colours.
- How the npm package ships `plugins-extra/` and how `ai plugin add cliproxy` finds it.
- Tests: fake plugin in `tests/check.sh` (rows, timeout, failure line, launch, add/rm, failover
  skip); Rust unit tests for parsing and failover exclusion.
- README section "Plugins" with the protocol and the cliproxy setup; AGENTS.md contract entry.

## Settled after the builder check

Answered by the conversation:
- **Plugin tabs are outside failover entirely** (user: "failover will not do"). A plugin tab gets no
  account dir, no failover mod, no handoff file, no limit detection; a limit inside it never moves
  anything. In the TUI it is a new, small tab kind; failover/WRR code paths filter it out.
- Tab and `.default` key: `plugin:<name>/<id>`.

My calls (check while reading):
- **Bash launch:** dispatch on the `plugin:` prefix in core (not `_ai_<provider>_run`). It goes
  through the same shared-trust step as other launches (exact folder, only when `trust.on`), no
  caffeinate change, then `exec "$dir/plugin" launch "$id" "$@"`.
- **Timeout on bash 3.2 / Git Bash:** pure-bash watchdog (background job + `sleep 2; kill`), no
  `timeout` binary. Windows: run as `bash "$dir/plugin"`, skip the executable-bit check there.
- **TUI refresh:** each plugin's `rows` runs on its own background thread; the home list shows the
  last good rows (first time: dim `loading…`) and never waits. Results replace on arrival. cwd for
  `rows` is the TUI's start folder.
- **Sections:** keyed by plugin name, not title (same titles are fine; title changes just re-label).
  Zero entries: section shows the header with a dim `no entries` line.
- **cliproxy key:** `config` is parsed as `key=value` lines (not sourced). `key=` wins over
  `key_env=`. The TUI and bash inherit the shell env, so `CLIPROXY_API_KEY` works when `ai` is
  started from a shell; the "no key" status names both options.
- **npm ships only the Rust binary**, so the TUI implements plugin add/update/rm/on/off in Rust
  (same steps and files as bash), and the bundled cliproxy plugin is embedded in the binary
  (`include_str!`, like the failover mod) and written out on `add cliproxy`. Bash `ai` reads it from
  `plugins-extra/cliproxy/`.
- `.default` readers already ignore unknown ids; a `plugin:` id that no longer resolves is ignored.
- AGENTS.md gets a "Plugins" contract (folder, protocol, `.off`, `.source`, `plugin:` ids) and the
  third network exception (explicit `ai plugin add|update` from git only).

Second check (actions and on/off), my calls:
- **`plugin do` in the TUI:** no PTY, not interactive (stdin closed). Runs on a background thread,
  stdout+stderr captured; the last line shows in the flash bar, the full output in a small window
  on `?`. On the 30 s timeout the process is killed. `rows` refreshes after every `do`, success or
  not. One `do` per plugin at a time (a second press says "busy").
- **Action keys:** not free-form. On a selected plugin row, `a` opens that plugin's action menu
  (labels from `plugin actions`; the key column is dropped from the protocol, lines are
  `<action><TAB><label>`). No collision with core keys. Bash picker: actions are CLI only
  (`ai plugin do <name> <action>`); `_ai_choose` gains no action.
- **`.off`:** `ai plugin off|on` and the TUI toggle both write/remove `<plugin>/.off`. Off blocks
  `rows`, `actions`, `do` and `launch`; a `.default` pointing at it is ignored. The off line uses
  the folder name (`<name> · off`), so no title cache is needed. `ai plugin ls` shows on/off.
- **cliproxy `start_cmd` / `stop_cmd`:** the config is parsed as `key=value`, never sourced; these
  two values run through `bash -c` like providers-file commands (user's own config).

## Revision: two sections, merged usage (user, after review)

The user wants, when the cliproxy plugin is on, two sections on the home list:
1. The existing failover section, unchanged: cla accounts, failover works as today.
2. The plugin's section, which looks like one more account ("all accounts merged") and shows usage
   for all of the proxy's accounts together, by model.

Both kinds of numbers are wanted (user: "1 and 2"):
- **Request / token counts per model**, from CLIProxyAPI's own usage statistics
  (`/v8/management/observability/usage/...`, management key, localhost only).
- **Limit bars merged across accounts**, like the bars cla accounts show (5h / weekly). Limits are
  per provider account, not per model, so these are one line per provider (Claude, Codex, Gemini…):
  the average used % across that provider's accounts, the soonest reset, and `n accts`. Source: the
  proxy's `/v8/management/credentials` list plus `/requests/api-call` to each provider's own usage
  endpoint (how the CLIProxyAPI Management Center reads quotas). A provider without a usage endpoint
  shows counts only.

Protocol change: an entry line may be followed by detail lines `  <TAB><text>` (two spaces then a
tab), shown indented under that entry in the TUI and the bash picker. Core does not interpret them.

Plugin config gains `mgmt_key_env=CLIPROXY_MGMT_KEY` (or `mgmt_key=`). Without it, the row shows
only up/down and a hint. Usage is fetched by the plugin at most once per 60 s and cached in
`$AI_PLUGIN_DIR/cache/usage`; `rows` always answers from the cache within the 2 s limit and
refreshes it in the background. Network: the plugin's own calls (to the localhost proxy, which then
calls the providers) — not core's.

To verify at build time (from CLIProxyAPI source, not guessed): the exact usage endpoint and whether
it gives a per-model breakdown; the `/requests/api-call` body; each provider's usage endpoint.
