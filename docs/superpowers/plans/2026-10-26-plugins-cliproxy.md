# User plugins + cliproxy plugin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Users install any plugin (folder / git / bundled) into ai-launcher; plugins show their own
section in the TUI and bash picker, outside failover; ship a cliproxy plugin giving one "all
accounts" Claude Code channel with merged usage.

**Architecture:** A plugin is a folder with an executable `plugin` speaking a line protocol (below).
Bash (`lib/plugins.sh`) and Rust (`tui/src/plugins.rs`) each implement the same protocol and the
same management steps on the same files. The cliproxy plugin is a bash script in
`plugins-extra/cliproxy/`, embedded in the TUI binary with `include_str!`.

**Tech Stack:** bash 3.2, Rust (ratatui, portable-pty), jq/curl optional.

**Spec:** `docs/superpowers/specs/2026-10-26-plugins-cliproxy-design.md` (read all of it,
including "Settled" and "Revision" sections; they override earlier text).

## Global Constraints

- All AGENTS.md rules (bash 3.2 list, Git Bash, `pad`, `has`, fake `$HOME` tests, no `timeout`
  binary, never lose user config).
- Plugins dir: `${AI_PLUGINS:-<dir of providers file>/plugins}`; plugin name `^[A-Za-z0-9_-]+$`;
  entry id `^[A-Za-z0-9._-]+$`; shared id `plugin:<name>/<id>`.
- Protocol (exact, both implementations):
  - `plugin rows` → line 1 `section\t<title>`; entry `<id>\t<label>\t<status>`; detail line
    `  \t<text>` (two spaces, TAB) belongs to the previous entry. Timeout 2 s. Failure/timeout →
    one dim line `<name>: plugin failed`. Zero entries → `no entries`.
  - `plugin actions` → `<action>\t<label>` lines. Timeout 2 s. Failure → none.
  - `plugin do <action>` → no tty, stdin closed, stdout+stderr captured, 30 s, killed on timeout.
  - `plugin launch <id> [args…]` → foreground with the terminal.
  - Env for every call: `AI_PLUGIN_DIR=<plugin folder>`; cwd = user's folder (TUI: start folder).
  - Windows: run as `bash <dir>/plugin …`; no exec-bit check there.
- Files: `<plugin>/.off` (empty marker = off), `<plugin>/.source` (one line: git URL used to install).
- Off: never run rows/actions/do/launch; section shows `<name> · off`; `.default` pointing at it ignored.
- Plugin rows/tabs: never in failover order, WRR, limited marks, handoff, failover mod.
- Network: only `ai plugin add|update` with a git source (`git clone --depth 1`, `git pull --ff-only`).
- Bundled plugins: `add cliproxy` copies `plugins-extra/cliproxy/` (bash) / writes embedded files (TUI).
- Key safety: no key ever printed; plugin config written mode 600.

## Review Focus

1. A plugin that hangs (sleep 100) or prints garbage → home/picker still renders within ~2 s, others unaffected. (Tasks 2, 3)
2. Plugin names/ids with `../`, spaces, `/` → rejected, never path-joined. (Tasks 2, 3)
3. `ai plugin rm` on a symlinked plugin folder → only the link removed. (Tasks 2, 3)
4. Proxy down / no keys / jq or curl missing → cliproxy rows still answer fast with a hint. (Task 4)
5. `.default` = `plugin:x/y` with x removed or off → plain `ai` falls back to the picker. (Tasks 2, 3)

---

### Task 1: CLIProxyAPI facts (research, no product code)

**Files:** Create `docs/superpowers/notes/cliproxyapi.md`

- [ ] From router-for-me/CLIProxyAPI source (gh api, not guesses): auth headers; client-key
  `/v1/models`; management usage endpoint(s) and response JSON with a per-model breakdown (requests,
  tokens) if any; `/v8/management/credentials` response (provider/type per credential, status);
  `/v8/management/requests/api-call` request body; per-provider upstream usage endpoints the
  Management Center (router-for-me/Cli-Proxy-API-Management-Center) calls for Claude/Codex/Gemini
  quotas and the fields that give used % + reset time for 5h / weekly. Also v0 equivalents.
- [ ] Each fact with a source link (file + line). Mark anything not found as NOT FOUND.

### Task 2: Bash plugin core

**Files:** Create `lib/plugins.sh`; Modify `lib/core.sh` (rows, picker dispatch, `ai_main`
subcommand, `.default`), `lib/failover.sh` only if needed to skip `plugin:` ids; Test `tests/check.sh`.

**Produces:** `_ai_plugins_dir`, `_ai_plugin_names` (on and off), `_ai_plugin_call <name> <secs> <args…>`
(watchdog: background job + `sleep; kill`, kills child tree), `_ai_plugin_rows` (picker rows,
provider field `plugin:<name>`, detail lines as non-selectable rows), `ai plugin ls|add|update|rm|on|off|do`.

- [ ] Tests first in `tests/check.sh` with fixture plugins written into the fake HOME:
  `good` (2 entries + detail line), `slow` (`sleep 30`), `bad` (exit 1), `off` (has `.off`):
  rows show sections in name order; slow shows `slow: plugin failed` and total time < 5 s; bad
  shows failed line; off shows `off · off` and is never executed (plugin writes a marker file if run);
  selecting `plugin:good`/`a` runs `plugin launch a` with args; `ai plugin add <folder>` /
  `add cliproxy` / refuse overwrite / refuse missing `plugin`; `add` from a local bare git repo
  (file:// URL) writes `.source`; `update` pulls; `rm` asks y/N, `AI_YES=1` skips, symlink case;
  names `../x`, `a/b`, `a b` rejected; `.default`=`plugin:good/a` launches it, removed/off falls
  back; failover order never lists plugin ids; `ai plugin do good x` captures output, 30 s kill
  (use `AI_PLUGIN_DO_TIMEOUT` env override to test with 1 s).
- [ ] Implement until `bash tests/check.sh` and `/bin/bash tests/check.sh` print `all passed`.
- [ ] Do not commit (main thread commits).

### Task 3: TUI plugin core

**Files:** Create `tui/src/plugins.rs`; Modify `tui/src/app.rs`, `tui/src/ui.rs`, `tui/src/config.rs`,
`tui/src/main.rs` (mod).

**Consumes:** `plugins-extra/cliproxy/plugin` and `plugins-extra/cliproxy/config.example` via
`include_str!` (main thread creates stubs before this task starts; Task 4 fills them).

**Produces:** `plugins::list(&Paths) -> Vec<Plugin>`, `plugins::parse_rows(&str) -> Rows`,
`plugins::run(dir, args, timeout) -> Result<String>`, `plugins::add/rm/set_on/update`.

- [ ] Unit tests first: parse_rows (section, entries, detail lines, junk lines ignored); timeout
  kills a `sleep 30` plugin within 3 s; name/id validation; add folder / embedded cliproxy / refuse
  overwrite; rm symlink removes link only; `.off` toggle; failover/WRR target lists never contain a
  plugin entry; `.default` resolves `plugin:x/y` only when present and on.
- [ ] Home: plugin sections after OTHER CLIS, header = title, entry line + indented detail lines,
  stale rows kept while a background refresh runs (first time `loading…`). Enter opens a tab running
  `plugin launch <id>` (new tab kind, no failover hooks). `a` on a plugin row opens its action menu,
  runs `do` on a thread, last output line in flash. Toggle on/off on the section header (click or
  `o`). "add plugin" form (one field: source) and delete with confirm, same as accounts.
- [ ] `cargo fmt --check && cargo clippy -- -D warnings && cargo test` pass. Do not commit.

### Task 4: cliproxy plugin

**Files:** Create `plugins-extra/cliproxy/plugin` (bash, exec bit), `plugins-extra/cliproxy/config.example`,
`tests/cliproxy.sh` (standalone, fake HOME, fake proxy = small `nc`-free bash/python http server
or a `curl` shim on PATH returning fixture JSON).

**Consumes:** `docs/superpowers/notes/cliproxyapi.md` (Task 1).

- [ ] Tests first: no config → `rows` creates `config` (mode 600) and shows key hint; proxy down →
  `proxy not running — start: brew services start cliproxyapi`; up + keys → entry `all` plus detail
  lines: one per provider with merged bars (avg %, soonest reset, `n accts`) and one per model
  (`<model>  <n> req  <tokens> tok`); `rows` answers < 2 s from cache while refresh runs; cache
  TTL 60 s; key never in any output; `launch all --foo` execs `claude --foo` with
  `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`
  and no `CLAUDE_CONFIG_DIR` (fake `claude` on PATH dumps env); launch with proxy down exits 1 with
  hint; `actions` lists start/stop only when brew has cliproxyapi or `start_cmd`/`stop_cmd` set;
  config parsed as key=value, `key=` wins over `key_env=`; works without jq (counts hidden, hint).
- [ ] `bash tests/cliproxy.sh` and `/bin/bash tests/cliproxy.sh` print `all passed`. Do not commit.

### Task 5: Wire-up, docs, screenshots, PR (main thread + helpers)

- [ ] `tests/check.sh` runs `tests/cliproxy.sh`; README "Plugins" section (install any plugin,
  protocol, on/off, actions, cliproxy setup); AGENTS.md: layout rows (`lib/plugins.sh`,
  `tui/src/plugins.rs`, `plugins-extra/`), Plugins contract, third network exception.
- [ ] Full checks: bash + /bin/bash + bash -n + cargo fmt/clippy/test.
- [ ] Sonnet whole-branch review; fix findings.
- [ ] Screenshots (fake data, fake proxy, `alice@example.com`): TUI home with failover section +
  CLIPROXY section with merged usage; plugin action menu; bash picker with plugin section. Attach to PR.
- [ ] Commit, push branch, open PR.
