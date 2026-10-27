# Plugin Install, Setup Scripts and Merged Plugins Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plugins can ship a reviewed-then-approved `setup` script that finishes installation for the user; the TUI gets one merged Plugins screen (search, installed, available, add); cliproxy gets a full setup; authors get a template and a README guide.

**Architecture:** Install becomes stage → (review → approve → run setup) → rename, in both `lib/plugins.sh` and `tui/src/plugins.rs`, with identical rules. The TUI's old Plugins window and separate add form merge into one screen state on `App`; setup runs in an ordinary TUI tab (`Pane`) whose exit code decides commit or rollback. cliproxy's `setup` is a bash script beside `plugin`.

**Tech Stack:** bash 3.2, Rust (ratatui, crossterm, the existing `Pane`), `tests/check.sh`, `tests/cliproxy.sh`, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-10-27-plugin-install-setup-design.md`

## Global Constraints

- Bash 3.2 rules from AGENTS.md (no `declare -A`, `mapfile`, `${v,,}`, etc.); run `bash tests/check.sh` and `/bin/bash tests/check.sh`; both print `all passed`.
- Rust: `cd tui && cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
- No emoji in the TUI. Allowed symbols: `✻ ● ○ ↻ ✎ ×`, `+`, `-`, `›`, `·`, `▌`, box drawing, plain text. The home button `⚙ Plugins` becomes ` Plugins `.
- `lib/plugins.sh` and `tui/src/plugins.rs` must match on: staging folder name `.<name>.install`, setup rules, `about` parsing, available-list rule.
- Never follow a symlink on rm; never overwrite an installed plugin; plugin names `[A-Za-z0-9_-]+`; folders starting with `.` are never listed as plugins.
- `setup` must be a regular file (not a symlink) inside the plugin folder; the reviewed bytes are copied to a temp file and run as `bash <copy>`, cwd = plugin folder, env `AI_PLUGIN_DIR` = plugin folder (staging during install), no timeout, with a tty.
- An install is kept only when setup exits 0; otherwise the staging folder is removed. Run setup again never rolls back and first copies `config` → `config.bak` when `config` exists.
- `update` never runs setup; if `setup` changed in the pull, print/show `setup changed: run setup again to apply`.
- `AI_YES=1` approves the review only (tests). cliproxy test-only env: `CLIPROXY_SETUP_ACCOUNTS=all|none`, `CLIPROXY_RELEASE_BASE` (release download base URL, `file://` in tests).
- Every new bundled file (cliproxy `setup`, `about`) is embedded in the TUI with `include_str!`.
- The launcher itself adds no network calls; cliproxy's download happens only inside an approved setup.

## Review Focus

1. A git plugin whose `setup` is a symlink (to `~/.ssh/...` or outside): install must refuse with `setup is a link: not run`, show nothing, run nothing. → tests in Task 1 and Task 3.
2. Setup interrupted (Ctrl-C in bash, tab closed in TUI): no `<plugins>/<name>` folder and no `.<name>.install` left behind. → Task 1 (kill test), Task 5 (closed-tab test).
3. A stale `.<name>.install` from a crashed earlier run: next install of the same name removes it first and proceeds; it is never listed as a plugin. → Task 1, Task 3.
4. Setup script with ANSI/control bytes: the review shows them escaped (`^[`), never raw. → Task 1 (bash `cat -v` style), Task 5.
5. cliproxy setup re-run with existing keys/accounts: keys unchanged, existing proxy accounts `kept`, no new login for them, proxy `config.yaml` only backed up once. → Task 6.

---

### Task 1: bash staging install, setup review/run, `ai plugin setup`

**Files:**
- Modify: `lib/plugins.sh` (`_ai_plugin_add`, `_ai_plugin_update`, dispatch line `ls|add|update|rm|on|off|do`)
- Test: `tests/check.sh` (user plugins section, after line ~357)

**Interfaces:**
- Produces:
  - `_ai_plugin_setup_ok <dir>`: returns 0 if `<dir>/setup` absent or a regular non-link file; else prints `setup is a link: not run`, returns 1.
  - `_ai_plugin_review <file>`: prints `SETUP SCRIPT · review before running` then the file with line numbers, control chars shown as `^X` (pipe through `cat -v`-equivalent `sed`); uses `less -R` only when stdout is a tty and `less` exists.
  - `_ai_plugin_run_setup <dir>`: copies `<dir>/setup` to `$(mktemp)`, runs `(cd <dir> && AI_PLUGIN_DIR=<dir> bash <copy>)`, removes the copy, returns setup's exit code.
  - `_ai_plugin_approve <dir>`: review + `run this setup script? [y/N]` (skipped when `AI_YES=1`), returns 0 on yes.
  - `ai plugin setup <name>`: `_ai_plugin_setup` — installed plugin with `setup`, approve, `cp config config.bak` if present, run; prints `setup failed (exit N)` on failure; plugin stays.
  - `_ai_plugin_add` behaviour: fetch into `<plugins>/.<n>.install` (remove a stale one first), existing checks run there, then if `setup` exists: `_ai_plugin_setup_ok`, `_ai_plugin_approve` (no → rm staging, print `cancelled: plugin not installed`, return 1), `_ai_plugin_run_setup` (non-zero → rm staging, print `setup failed (exit N): plugin not installed`, return 1). Then `mv` staging → `<plugins>/<n>`, rest unchanged (`.off` removed, `rows` warm, `installed plugin …`). `.source` written in staging before setup. A `trap` on INT/TERM inside add removes staging.
  - `_ai_plugin_update`: record `cksum` of `setup` before the pull; if it differs after, print `setup changed: run setup again to apply`.

- [ ] **Step 1: Write failing tests** in `tests/check.sh` using a local folder plugin `$T/src/withsetup` whose `setup` writes `$AI_PLUGIN_DIR/config` and `pwd` into `$T/setup.log`:
  - `ok "setup: approved runs in staging, then installs"` — `AI_YES=1 _ai_plugin_add "$T/src/withsetup"`; `$PL/withsetup/config` exists; log's pwd ends in `/.withsetup.install`; no `$PL/.withsetup.install`.
  - `ok "setup: refused leaves nothing"` — `echo n | _ai_plugin_add …` returns 1; neither `$PL/withsetup2` nor `$PL/.withsetup2.install` exist.
  - `ok "setup: failing setup leaves nothing"` — setup `exit 3`; output contains `setup failed (exit 3): plugin not installed`; no folders left.
  - `ok "setup: a linked setup is never run"` — `setup` is `ln -s "$T/evil"`; output `setup is a link: not run`; `$T/evil.ran` absent; nothing installed.
  - `ok "setup: review escapes control bytes"` — setup contains `printf '\033[2J'` literal ESC byte; `_ai_plugin_review` output contains `^[` and no raw `\033`.
  - `ok "setup: stale staging is replaced"` — pre-create `$PL/.withsetup3.install/junk`; install succeeds; `junk` absent in result; `_ai_plugin_names` never lists a dot folder.
  - `ok "setup: plugin setup reruns and keeps config.bak"` — `AI_YES=1 _ai_plugin_setup withsetup`; `config.bak` equals the previous `config`.
  - `ok "setup: plugin without setup installs as before"` — existing add tests still pass unchanged.
  - `ok "setup: update says setup changed"` — file:// git plugin, change `setup` upstream, `_ai_plugin_update name` prints `setup changed: run setup again to apply`.
  - `ok "setup: interrupt removes staging"` — setup `sleep 30`; run add in background with `AI_YES=1`, `kill -INT` it after 1 s; no staging or plugin folder left.
- [ ] **Step 2: Run** `bash tests/check.sh` — the new lines print `FAIL`.
- [ ] **Step 3: Implement** the interfaces above in `lib/plugins.sh`; add `setup` to the dispatch list.
- [ ] **Step 4: Run** `bash tests/check.sh` and `/bin/bash tests/check.sh` — `all passed`; `bash -n lib/plugins.sh`.
- [ ] **Step 5: Commit** `feat(plugins): setup scripts with review, staging and rollback (bash)`.

### Task 2: bash available list, `about`, template, `ai plugin new`

**Files:**
- Create: `templates/plugin/plugin`, `templates/plugin/setup`, `templates/plugin/about`, `templates/plugin/README.md`
- Modify: `lib/plugins.sh` (`_ai_plugin_ls`, dispatch), `plugins-extra/cliproxy/about`
- Test: `tests/check.sh`

**Interfaces:**
- Consumes: Task 1 add/staging.
- Produces:
  - `_ai_plugin_about <dir>`: first line of `<dir>/about`, control chars stripped, max 60 chars; empty if missing.
  - `_ai_plugin_available`: one line per bundled plugin (`$AI_HOME/plugins-extra/*`) not installed: `<name><TAB><about><TAB>setup|-`.
  - `ai plugin ls`: existing installed table, then `available` header and the `_ai_plugin_available` lines (`has setup` for `setup`), or `no other bundled plugins`.
  - `ai plugin new <name>`: `_ai_plugin_new` — name check, refuses existing, copies `$AI_HOME/templates/plugin` into `<plugins>/<name>`, `chmod +x plugin setup`, prints `created plugin <name> in <dir>: edit plugin, then run ai`. Does not run setup.
  - `plugins-extra/cliproxy/about`: `Claude Code through CLIProxyAPI: all accounts as one`.
  - Template `plugin`: commented bash implementing `rows` (section line + one entry `hello` + one detail line), `actions` (`ping<TAB>Ping`), `do ping`, `launch hello` (`exec bash -c 'echo hello from $AI_PLUGIN_DIR; exec "${SHELL:-bash}"'`). Comments state the protocol rules: rows fast, no prompts, plain text, never print secrets, state relative to `AI_PLUGIN_DIR`.
  - Template `setup`: `set -e`, idempotent example (create `config` only if missing, mode 600), clear `echo` per step, comment that it runs once with a tty after the user approves it.

- [ ] **Step 1: Failing tests:**
  - `ok "plugin new: creates a working plugin"` — `_ai_plugin_new demo`; `$PL/demo/plugin rows` first line is `section${TAB}…`; picker rows contain `plugin:demo${TAB}hello`.
  - `ok "plugin new: refuses existing and bad names"` — second `new demo` and `new "a b"` fail; demo untouched.
  - `ok "plugin ls: lists available bundled with about"` — with cliproxy not installed, `ai plugin ls` shows `cliproxy` under `available` with the about text and `has setup` (after Task 6 adds the setup; until then assert the about text only).
  - `ok "plugin ls: installed bundled leaves available"` — after `AI_YES=1 ai plugin add cliproxy`, the line `no other bundled plugins` (cliproxy's setup is stubbed in tests by `CLIPROXY_SETUP_ACCOUNTS=none` + fakes from Task 6; until Task 6 there is no setup).
  - `ok "template setup is safe to run twice"` — run `$PL/demo/setup` twice with `AI_PLUGIN_DIR=$PL/demo`; exit 0 both; `config` unchanged after the second run.
- [ ] **Step 2: Run** `bash tests/check.sh` — FAIL lines.
- [ ] **Step 3: Implement** the files and functions above; add `new` to dispatch.
- [ ] **Step 4: Run** both test commands — `all passed`.
- [ ] **Step 5: Commit** `feat(plugins): plugin template, ai plugin new, available list`.

### Task 3: Rust plugins.rs — staging, setup file rules, available list

**Files:**
- Modify: `tui/src/plugins.rs` (`add`, `update`, `list`, the `CLIPROXY` const), tests module in the same file.

**Interfaces:**
- Consumes: the rules from Task 1/2 (must match).
- Produces:
  - `pub struct Staged { pub name: String, pub dir: PathBuf, pub setup: Option<String> }` — `dir` is `<plugins>/.<name>.install`; `setup` holds the exact text to review (None when no setup).
  - `pub fn stage(paths: &Paths, source: &str) -> Result<Staged, String>` — current `add` logic writing into staging (remove a stale staging first); runs `setup_text`.
  - `pub fn setup_text(dir: &Path) -> Result<Option<String>, String>` — `Err("setup is a link: not run")` for a symlink, `Ok(None)` when absent.
  - `pub fn setup_copy(text: &str) -> Result<PathBuf, String>` — writes the reviewed text to a temp file (the run uses these exact bytes).
  - `pub fn commit(paths: &Paths, s: &Staged) -> Result<String, String>` — rename staging → `<plugins>/<name>`, remove `.off`, warm `rows` (10 s), return the same `installed plugin …` message as today.
  - `pub fn abort(s: &Staged)` — remove staging (never follow a link).
  - `pub fn add(paths, source)` stays for no-setup sources: `stage` then `commit`; returns `Err("needs setup")`-free behaviour by being called only when `setup` is None (the TUI decides).
  - `pub fn backup_config(dir: &Path)` — `config` → `config.bak` if present.
  - `pub struct Available { pub name: String, pub about: String, pub has_setup: bool }`; `pub fn available(paths: &Paths) -> Vec<Available>` — bundled not installed.
  - `pub fn about(dir: &Path) -> String` — same rule as bash (first line, control chars stripped, 60 chars).
  - `list` skips names starting with `.` (already enforced by `valid_name`; add a test).
  - `update` returns `… · setup changed: run setup again to apply` when `setup` bytes changed.
  - `CLIPROXY` gains `("about", include_str!(…/about), false)`; Task 6 adds `setup`.

- [ ] **Step 1: Failing tests** in `plugins.rs` tests: `stage_then_commit_installs`, `stage_with_setup_returns_text_and_installs_nothing`, `abort_removes_staging`, `setup_link_is_refused`, `stale_staging_is_replaced`, `dot_folders_are_never_listed`, `available_lists_bundled_not_installed_with_about`, `update_reports_setup_changed` (file:// repo, as existing git tests do), `backup_config_copies_config`.
- [ ] **Step 2: Run** `cd tui && cargo test plugins::` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cd tui && cargo fmt --check && cargo clippy -- -D warnings && cargo test` — pass.
- [ ] **Step 5: Commit** `feat(tui): plugin staging, setup rules and available list`.

### Task 4: TUI merged Plugins screen

**Files:**
- Modify: `tui/src/app.rs` (`plugins_win`, `plugins_win_key`, mouse handling ~1063-1094, `P` key, tests ~2759-2894), `tui/src/ui.rs` (`plugins_window` ~931, home button label ~860, footer ~1310), `tui/src/form.rs` (`Purpose::AddPlugin` form hosted in the screen)

**Interfaces:**
- Consumes: `plugins::available`, `plugins::about`, `plugins::stage/commit/abort`, `Staged` (Task 3).
- Produces:
  - `pub struct PluginsWin { pub sel: usize, pub search: String, pub searching: bool, pub form: Option<Form>, pub review: Option<Review> }` replacing `plugins_win: Option<usize>` with `plugins_win: Option<PluginsWin>`. `Review` is defined in Task 5; in this task `review` stays `None`.
  - `fn plugins_items(&self) -> Vec<Item>` where `enum Item { Installed(usize), Available(usize), AddButton }` — filtered by `search` (case-insensitive substring of name or about), in order: installed, available, add button.
  - Keys (screen, not searching): Up/Down/j/k move; `/` start search (typing edits `search`, Enter/Up/Down leave search, Esc clears search text then leaves search); Enter on installed = actions (as today `a`); `space` and `o` on/off; `u` update; `s` run setup (Task 5); `-` remove (existing confirm); `+` on Available = install, elsewhere = open the add form inline; Esc closes the innermost: form, then search text, then screen. `q` closes only when not searching.
  - Install from `+` / form submit: `stage`; `setup == None` → `commit`, notice; `Some(text)` → hand to Task 5 (`self.open_review(staged)`); in this task, until Task 5, `abort` with notice `setup scripts need the review screen` — replaced in Task 5.
  - The old home-wide `p` add key and `+ Plugin` button open this screen with the form open (same entry point, no separate window).
  - Layout per spec mockup: title `Plugins` with `N installed · M available`; `search:` line; `INSTALLED` rows (name, about/title, status `● ok`/`× failed`/`·`/`loading…`, `● on`/`○ off`, `actions`, `↻ update` for git plugins, `[-]`); `AVAILABLE · bundled` rows (`has setup`, `[+]`) or `no other bundled plugins`; `[+ Add from git / folder]`; footer `enter actions   space on/off   u update   s setup   - remove   + install/add   / search   esc back`. All `[+]`, `[-]`, `actions`, `↻ update`, on/off, add button clickable via `hits`.
  - Home button label ` Plugins ` (no `⚙`).

- [ ] **Step 1: Failing tests** (rewrite the existing `plugins_window_*` tests to the new state, keep their behaviour): `plugins_screen_lists_installed_then_available_then_add`, `search_filters_both_lists_by_name_and_about`, `esc_closes_form_then_search_then_screen`, `minus_asks_then_removes`, `plus_on_available_without_setup_installs`, `plus_on_installed_opens_add_form`, `space_toggles_off_file`, `home_button_has_no_emoji` (assert label == `" Plugins "` and no char in all button labels is outside the allowed set), `available_empty_says_no_other_bundled_plugins`.
- [ ] **Step 2: Run** `cd tui && cargo test` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** full Rust check — pass. Manually: `cargo run`, press `P`, check the layout against the spec mockup at 80 and 120 columns.
- [ ] **Step 5: Commit** `feat(tui): one Plugins screen with search, install and remove`.

### Task 5: TUI setup review screen and setup tab

**Files:**
- Modify: `tui/src/app.rs` (review state/keys, setup tab launch beside `launch_plugin` ~1376, `tick` exit handling), `tui/src/ui.rs` (review render)

**Interfaces:**
- Consumes: `Staged`, `setup_copy`, `commit`, `abort`, `backup_config`, `setup_text` (Task 3); `PluginsWin` (Task 4); `Pane::spawn`, `Pane::exited`.
- Produces:
  - `pub struct Review { pub target: SetupTarget, pub text: String, pub scroll: usize }`; `pub enum SetupTarget { Install(Staged), Rerun(String /* plugin name */) }`.
  - `fn open_review(&mut self, target: SetupTarget)`; keys: Up/Down/PgUp/PgDn scroll, Enter or click `[ Approve and install ]` approves (label `[ Approve and run ]` for Rerun), Esc or `[ Cancel ]` cancels (Install → `abort`, notice `cancelled: plugin not installed`).
  - Approve: `setup_copy(text)` → open a new tab named `setup:<name>` running `bash <copy>` with cwd and `AI_PLUGIN_DIR` = staging (Install) or plugin dir (Rerun, after `backup_config`). The exit code comes from `Pane::exited()` (no wrapper file needed; this satisfies the spec's "exit code reported back"). Closing the tab before exit counts as failure.
  - On exit (in `tick`): Install + 0 → `commit`, reload plugins, notice `installed plugin <name>`; Install + non-zero/closed → `abort`, notice `setup failed (exit N): plugin not installed`; Rerun → notice `setup done` / `setup failed (exit N)`, plugin kept. The temp copy is removed either way.
  - `s` on an installed plugin with a `setup` (Task 4 key) → `setup_text` → `open_review(Rerun)`; plugins without setup show notice `no setup script`.
  - Render per spec mockup: header `Plugins › install <name>`, `SETUP SCRIPT · review before running` and `setup · N lines`, bordered numbered script with control chars as `^X`, the two notice lines, the two buttons, footer `enter approve   ↑↓ PgUp PgDn scroll   esc cancel`.

- [ ] **Step 1: Failing tests:** `plus_on_setup_plugin_opens_review_and_installs_nothing_yet`, `review_cancel_aborts_staging`, `review_shows_control_bytes_escaped`, `approve_opens_setup_tab_in_staging` (tab named `setup:<name>`, env `AI_PLUGIN_DIR` = staging), `setup_exit_zero_commits` and `setup_exit_nonzero_aborts` (drive with a setup script `exit 0`/`exit 4` and loop `tick` until done, as other pane tests do), `closing_setup_tab_aborts`, `rerun_backs_up_config_and_keeps_plugin_on_failure`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement**; replace Task 4's temporary `setup scripts need the review screen` branch with `open_review`.
- [ ] **Step 4: Run** full Rust check — pass. Manually install a local plugin with a setup that asks `read -p` for input and confirm input works in the tab.
- [ ] **Step 5: Commit** `feat(tui): review and run plugin setup scripts in a tab`.

### Task 6: cliproxy `setup`

**Files:**
- Create: `plugins-extra/cliproxy/setup`
- Modify: `plugins-extra/cliproxy/plugin` (config keys `proxy_config`, `bin`; `start`/`stop` use `start_cmd`/`stop_cmd` already), `plugins-extra/cliproxy/config.example`, `tui/src/plugins.rs` (`CLIPROXY` += `setup`, executable `true`)
- Test: `tests/cliproxy.sh`

**Interfaces:**
- Consumes: plugin's existing `apply`/`affinity` actions (`"$AI_PLUGIN_DIR/plugin" do apply`, `… do affinity`), config parsing rules (key=value, never sourced).
- Produces: `setup` steps, each printing `[n/4] <Step> <detail> .... <result>`:
  1. **Find/install.** Order: `cliproxyapi` on PATH → `brew list cliproxyapi` → `$AI_PLUGIN_DIR/bin/cliproxyapi` → proxy answers at `url`. Found → `use it`. Else brew → print `brew install cliproxyapi`, ask `[i]nstall / [m]yself`; no brew → print the release asset name and target `bin/`, ask `[d]ownload / [m]yself`. `m` → print command/link, exit 2. Download: base `${CLIPROXY_RELEASE_BASE:-https://github.com/router-for-me/CLIProxyAPI/releases/latest/download}`; resolve version from the `checksums.txt` asset names; asset `CLIProxyAPI_<ver>_<os>_<arch>.tar.gz` (`.zip` on Windows; os from `uname -s` lowercased `darwin|linux|windows`; arch `aarch64` for arm64/aarch64, `amd64` for x86_64); verify sha256 (`shasum -a 256` or `sha256sum`) against `checksums.txt`, refuse on mismatch or missing line; unpack into `bin/`; write `start_cmd=` (background run with pid file `run/pid`, log `run/log`, `-config <proxy_config>`) and `stop_cmd=` (kill pid) into `config`.
  2. **Keys.** If plugin `config` has `key=` and `mgmt_key=`: `kept`. Else generate 32-byte hex keys from `/dev/urandom` (`od -An -tx1`), find the proxy `config.yaml` (`proxy_config=` key; else brew prefix `etc/cliproxyapi.conf` or `etc/cliproxyapi/config.yaml` — builder confirms from the formula; else `$AI_PLUGIN_DIR/run/config.yaml` copied from the release's example), back it up once to `config.yaml.ai-launcher.bak`, set `secret-key` only when its value is empty (`secret-key: ""` → the key); if already set and plugin has no `mgmt_key`, prompt `paste the management key:`. Client key added through `PUT/PATCH` management `access.api-keys` after start (append, never replace), or into YAML only when `api-keys` is empty. Write `key=`/`mgmt_key=`/`proxy_config=` into plugin `config` (mode 600) by rewriting only those lines.
  3. **Accounts.** List launcher accounts: each `cla` account (`<cla root>/<name>`, email from its `.claude.json` `oauthAccount.emailAddress` via jq, else `?`) and each codex provider in the providers file. Proxy accounts from `GET /v0/management/auth-files` (emails). Checklist (fzf `--multi` with all preselected via `--bind start:select-all` when fzf exists; else numbered list: `1 3`, `a`, `n`, Enter). Already-in-proxy emails show `kept` and are not selectable. Enter with nothing ticked and no proxy account → `pick at least one account` and redisplay. For each ticked: print `sign in as <email>`, run `<cliproxyapi> -config <proxy_config> -claude-login` or `-codex-device-login`; after, re-read auth-files; if the new email differs, warn `signed in as X, expected Y: kept`. `CLIPROXY_SETUP_ACCOUNTS=all|none` skips the UI. (Start runs before logins when the management API is needed for the list: order is install → keys → start → accounts → apply; the printed numbering stays `[1/4] Install [2/4] Keys [3/4] Accounts [4/4] Start` with Start shown as `proxy up` once confirmed.)
  4. **Start.** Skip if the proxy answers. Else `brew services start cliproxyapi` or `start_cmd`; wait up to 10 s for an answer (`curl -s -m 1`); fail with the log tail. Then `plugin do apply` and `plugin do affinity`. Final line `Done. "Claude Code · all accounts" is ready.`
- Re-runnable: every step prints `done`/`kept`/`use it` when already true.

- [ ] **Step 1: Failing tests** in `tests/cliproxy.sh` with fakes on `$T/bin` (`brew`, `cliproxyapi` that appends its args to `$T/cpa.log` and writes a fake auth file for logins; reuse the existing python fake proxy, adding `GET /v0/management/auth-files` and the api-keys endpoint):
  - `ok "setup: uses an existing cliproxyapi"` — fake on PATH; no `brew install` in `$T/brew.log`.
  - `ok "setup: brew install when missing and chosen"` — no cliproxyapi, fake brew, answer `i`; `$T/brew.log` has `install cliproxyapi`.
  - `ok "setup: myself exits 2 and prints the command"`.
  - `ok "setup: no brew downloads and verifies the release"` — `CLIPROXY_RELEASE_BASE=file://$T/rel` with a tarball + good `checksums.txt`; `bin/cliproxyapi` exists; `config` has `start_cmd=`.
  - `ok "setup: bad checksum refuses"` — wrong hash; non-zero exit; no `bin/cliproxyapi`.
  - `ok "setup: keys created once, kept on rerun"` — run twice; `key=`/`mgmt_key=` identical; `config` mode 600; proxy yaml backup exists once and `secret-key` set only when empty.
  - `ok "setup: existing secret-key is never replaced"` — yaml has a bcrypt-looking value; answer the paste prompt; yaml unchanged.
  - `ok "setup: accounts all logs in each launcher account not in proxy"` — two fake cla accounts, proxy already has one email; `CLIPROXY_SETUP_ACCOUNTS=all`; `$T/cpa.log` has exactly one `-claude-login`.
  - `ok "setup: codex uses device login"`.
  - `ok "setup: never prints a key"` — setup output contains neither generated key.
  - `ok "setup: rerun is idempotent"` — second run with `CLIPROXY_SETUP_ACCOUNTS=all` makes no new login, prints `kept`, exit 0.
- [ ] **Step 2: Run** `bash tests/cliproxy.sh` — FAIL.
- [ ] **Step 3: Implement** `setup`, config keys, `about`/`setup` embedding in `CLIPROXY`.
- [ ] **Step 4: Run** `bash tests/cliproxy.sh`, `/bin/bash tests/cliproxy.sh`, `bash tests/check.sh`, full Rust check — all pass. Also update the Task 2 ls assertion to expect `has setup` for cliproxy.
- [ ] **Step 5: Commit** `feat(cliproxy): setup installs, keys, logs in and starts the proxy`.

### Task 7: README developer guide and AGENTS.md

**Files:**
- Modify: `README.md` (Plugins section, "Writing a plugin", cliproxy "Needs …" paragraph), `AGENTS.md` (Layout table: `templates/plugin/`; Plugins contract)

**Interfaces:**
- Consumes: final names from Tasks 1–6.

- [ ] **Step 1: README**:
  - Plugins usage: the merged screen (keys, `[+]`/`[-]`, search), `ai plugin setup|new`, review → approve → run, rollback, `available` in `ls`.
  - "Writing a plugin" for developers and AI agents: quick start (`ai plugin new demo`, edit, open `ai`); protocol table (existing); `setup` contract (regular file, runs once with a tty after approval, `AI_PLUGIN_DIR`, exit 0 = installed, must be re-runnable, never overwrite user config, state relative to `AI_PLUGIN_DIR`); `about` contract; rules checklist (rows fast and never prompt, plain text, never print secrets, no network in `rows` unless the plugin's purpose needs it and it is cached); testing by hand (`AI_PLUGIN_DIR=$PWD ./plugin rows`) and with a fake environment like `tests/cliproxy.sh`; publishing (git repo, `ai plugin add owner/repo`).
  - cliproxy: replace "Needs CLIProxyAPI installed, logged in and running … does not install it" with what setup does (find/install/download with checksum, keys, account checklist with a fresh login per account and why, start), and that env-var keys still work.
- [ ] **Step 2: AGENTS.md**: Plugins contract adds `setup` (rules above), `about`, `.<name>.install` staging (never listed), `ai plugin setup|new`, bash/Rust must match on these; Layout row for `templates/plugin/`; network rule note that cliproxy's download runs only inside an approved setup.
- [ ] **Step 3: Verify** every command and key in the docs exists: `grep` each `ai plugin <sub>` against the dispatch line and each TUI key against `plugins_win_key`.
- [ ] **Step 4: Commit** `docs: plugin setup, Plugins screen and a guide for plugin authors`.
