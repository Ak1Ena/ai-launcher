# Plugin install, setup scripts, and the merged Plugins screen

Date: 2026-10-27. Builds on `2026-10-26-plugins-cliproxy-design.md` (the plugin protocol and the cliproxy plugin, already shipped on `feat/plugins`).

## Intent

Today a plugin is hard to use. Installing one means typing a source into a separate add form, and then the user still has to set things up by hand. For cliproxy that is: install CLIProxyAPI, log in to each account, invent keys, export them as env vars, and start the service. The user wants:

1. **One Plugins screen** that combines plugin settings and adding plugins: a search box, the installed plugins with their controls, the bundled plugins not yet installed with an install button, and a button to add one from git or a folder.
2. **Setup scripts.** A plugin may ship a setup script. Install shows the script, asks for approval, then runs it and finishes the whole setup for the user. The user can always read exactly what will run.
3. **A plugin template** so other people can write plugins for the launcher.
4. **A developer guide in the README** written for both human developers and future AI agents.

Success: a user presses `+` on cliproxy, reads the script, approves it, ticks their accounts, signs in once per account, and ends with a working "Claude Code · all accounts" row. No file editing and no env vars.

## Goals

- Merged Plugins screen in the TUI (search, installed, available, add).
- Optional `setup` file in a plugin; review, then approve, then run, on install and on demand.
- An install is kept only if setup succeeds; cancel or failure leaves nothing behind.
- cliproxy `setup` that does install, keys, accounts and start end to end.
- `ai plugin new <name>` from a template, plus a README guide for plugin authors.
- Same behaviour in bash `ai` (as text prompts) and the TUI, as for every other plugin feature.

## Non-goals

- No online plugin registry or catalog. "Available" means bundled plugins only (the no-network rule stays: git only on an explicit add/update).
- No model picker in setup. The proxy routes by model name; users switch with `/model` inside Claude Code.
- No Gemini login in cliproxy setup (the current CLIProxyAPI version has no Gemini login flag).
- No reuse or copy of existing `cla` / codex login tokens into the proxy (see Decisions).
- No sandboxing of plugin code. Review is for transparency, not containment.

## Anti-goals (would make this a failure even if it works)

- Emoji in the TUI. Use only symbols the TUI already uses (`✻ ● ○ ↻ ✎ ×`), `+`, `-`, and plain text. The existing `⚙ Plugins` button becomes `Plugins`.
- Running any setup code the user has not seen and approved.
- Breaking a user's existing logins: the launcher's `cla` accounts must keep working after cliproxy setup.
- Losing or overwriting user config (the proxy's `config.yaml`, existing keys, existing proxy accounts).
- A half-installed plugin folder after cancel or failure.

## Constraints (from AGENTS.md, unchanged)

- Bash 3.2, nothing to install; fzf/jq/tmux optional with graceful fallback. Git Bash on Windows.
- TUI and bash share the same plugin folder and files; new state goes in new files.
- `lib/plugins.sh` and `tui/src/plugins.rs` must match on add rules, name checks, and now setup rules.
- The TUI binary ships bundled plugins with `include_str!` (npm has no repo), so every new bundled file (`setup`, `about`, template files) must be embedded too.
- Never follow a symlink on `rm`.

## Decisions

### 1. Plugin folder additions (user + my call)

| File | Required | Meaning |
| --- | --- | --- |
| `plugin` | yes | Unchanged protocol. |
| `setup` | no | Setup script. Run with a real terminal (tty, stdin), no timeout, cwd = plugin folder, env `AI_PLUGIN_DIR` set. Exit 0 = success. Must be safe to run again. |
| `about` | no | One line of plain text: the description shown in the Plugins screen and in search. *My call:* a static file, so an uninstalled plugin can be described without running its code. |

`plugin` gets one more optional command? **No** (my call). Setup is a separate file so it can be shown in full and run without the 2 s / 30 s protocol limits.

### 2. Install flow (user decided the shape; details my call)

1. Get the files into a staging folder `<plugins>/.<name>.install/` (bundled copy, folder copy, or git clone). Names starting with `.` are never listed as plugins.
2. If there is no `setup`: move staging to `<plugins>/<name>/` and continue as today (start on, run `rows` once).
3. If there is a `setup`: show it in full (review screen below). Cancel removes the staging folder; nothing is installed.
4. Approve runs `setup` in the staging folder with `AI_PLUGIN_DIR` pointing at it. The TUI runs it in its own tab, like a launch, so prompts, fzf and browser logins work. Bash runs it in the foreground.
5. Exit 0: rename staging to `<plugins>/<name>/`, then continue as in step 2. Non-zero exit or interrupt: remove staging and show `setup failed (exit N): plugin not installed`.

The review covers `setup` only. The screen says that the plugin's own code also runs with the user's permissions.

Rules that keep the approval honest (my calls):
- `setup` must be a regular file inside the plugin folder, never a symlink; otherwise install stops with `setup is a link: not run`.
- The bytes shown are the bytes run: after review, `setup` is copied to a temp file and that copy runs with `bash <copy>` (no exec bit needed, the same on Windows).
- Plugin state must be relative to `AI_PLUGIN_DIR`, never an absolute staging path, because staging is renamed after setup. `plugin` must work when called from the staging folder (setup may call `"$AI_PLUGIN_DIR/plugin" do apply`). The template and the README say so.
- The rollback covers only the plugin folder. What setup did outside it (a `brew install`, proxy logins) stays, which is fine because setup is re-runnable.
- TUI: the setup tab runs a small wrapper that writes the exit code to `<staging>/.setup-exit`. Closing the tab before it is written counts as failed.
- Run setup again works in the live folder and never rolls back. It copies the plugin's `config` to `config.bak` first (if present).
- Tests: `AI_YES=1` approves the review only. A setup that needs choices reads a test-only env var (cliproxy: `CLIPROXY_SETUP_ACCOUNTS=all|none`).

**Run setup again:** `ai plugin setup <name>` and a **Run setup again** entry on installed plugins that have a `setup`. It shows the same review screen and then runs in the installed folder. A failure here keeps the plugin installed.

**Update** (`git pull`) never runs setup by itself. *My call:* if `setup` changed in the pull, print/show `setup changed: run setup again to apply` and leave it to the user.

### 3. TUI Plugins screen (user decided; mockup agreed)

Opened with `P` or the `Plugins` button on home. It replaces both the old Plugins window and the separate `p` add form: one screen.

```
 ✻ home   Plugins                                          3 installed · 2 available

  search: _                                                               [ / ]

  INSTALLED
 ▌ cliproxy     Claude Code · all accounts         ● ok        ● on    actions   [-]
   my-tool      Team gateway                       × failed    ● on    ↻ update  [-]
   old-thing    Legacy proxy                       ·           ○ off             [-]

  AVAILABLE · bundled
   example      (illustrative: today cliproxy is the only bundled plugin)  has setup   [+]

  [+ Add from git / folder]

  enter actions   space on/off   u update   s setup   - remove   + install/add   / search   esc back
```

- `/` focuses search; typing filters both lists by name and `about` text. Esc always closes the innermost thing first: the add form, then the search text, then the screen.
- No new bundled plugin is in scope; the available list will often be empty once cliproxy is installed, and then shows `no other bundled plugins`.
- `-` removes the plugin under the cursor (still asks y/N; same rules as today). `+` installs the available plugin under the cursor, or opens the add form when the cursor is on the add button or an installed row.
- The add form (git URL / `owner/repo` / folder path) is shown inside this screen, not as a separate window.
- Mouse: `[+]`, `[-]`, `actions`, `↻ update` and on/off are clickable, like the home buttons.
- Plugin rows on the home tab keep their existing keys.

Setup review screen:

```
 ✻ home   Plugins › install cliproxy

  SETUP SCRIPT · review before running                          setup · 142 lines

  ┌────────────────────────────────────────────────────────────────────────────┐
  │  1  #!/usr/bin/env bash                                                    │
  │  2  set -e                                                                 │
  │  …                                                              ↑↓ scroll  │
  └────────────────────────────────────────────────────────────────────────────┘

  Runs in a terminal tab as you. The plugin is installed only if setup succeeds.
  The plugin's own code also runs with your permissions: install only plugins you trust.

  [ Approve and install ]   [ Cancel ]

  enter approve   ↑↓ PgUp PgDn scroll   esc cancel
```

Control characters in the script are shown escaped, never sent to the terminal.

### 4. Bash `ai` (my call, mirrors the TUI)

- `ai plugin ls` adds an `available` section (bundled, not installed, with `about` and `has setup`).
- `ai plugin add <src>` with a `setup`: prints the script with line numbers (through `less -R` if present and stdout is a tty, else plain), then `run this setup script? [y/N]`. Same staging, approval and rollback as above. `AI_YES=1` approves (tests only; documented as such).
- `ai plugin setup <name>`: review, then approve, then run, on an installed plugin.
- `ai plugin new <name>`: see Template.
- The bash picker is unchanged; it has no Plugins screen.

### 5. cliproxy setup (user agreed)

Steps, each skipped and shown as `done` when it is already true:

```
  CLIProxyAPI setup
  [1/4] Install    found /opt/homebrew/bin/cliproxyapi ........... use it
  [2/4] Keys       client key + management key ................... created
  [3/4] Accounts   (checklist below)
  [4/4] Start      brew services start cliproxyapi ............... proxy up
  Done. "Claude Code · all accounts" is ready.
```

1. **Find or install CLIProxyAPI (user decided).** Use an existing install first, in this order: `cliproxyapi` on PATH, `brew list cliproxyapi`, the plugin's own `bin/` copy, a proxy already answering at `url`. Found means `use it`, no reinstall. Not found:
   - **brew present:** show `brew install cliproxyapi` with `[ Install ]` / `[ I'll install it myself ]`.
   - **no brew** (Linux, Windows, a Mac without brew): show the release download with `[ Download ]` / `[ I'll install it myself ]`. Download fetches the latest GitHub release asset for the OS/arch (`CLIProxyAPI_<ver>_<os>_<arch>.tar.gz`, `.zip` on Windows; arch names `aarch64` / `amd64`; checked 2026-10-27 at v8.0.44), verifies it against the release's `checksums.txt` (refuse on mismatch or missing entry), and unpacks it into `$AI_PLUGIN_DIR/bin/`. Then setup fills `start_cmd` / `stop_cmd` to run that binary as a background process (pid and log files in the plugin folder), because there is no `brew services`.
   - **I'll install it myself** prints the command or link and exits non-zero; Run setup again picks the install up later.
   - Any install/download failure prints the error and stops setup.
   - Setup never installs Homebrew itself (needs admin, changes the whole machine).
   - This network use happens only inside a setup the user reviewed and approved; the launcher itself still makes no new network calls.
2. **Keys.** Generate a random client key and management key only when none are set. Write the management key into the proxy's `config.yaml` (the proxy bcrypt-hashes it in place on start, so the plugin's `config` keeps its own plain copy as `mgmt_key=`, with the client key as `key=`; mode 600). Never replace an existing `secret-key`/`api-keys`: if the proxy already has a hashed management key the plugin does not know, ask the user to paste it. Back up `config.yaml` once before the first edit. Add the client key through the management API after start if that avoids editing YAML (builder's choice).
3. **Accounts.** A checklist. Each ticked account gets its **own fresh login** of the same account (same email and plan, a separate session for the proxy):

   ```
     [3/4] Accounts                                           3 of 3 selected

           FROM AI-LAUNCHER                         sign in as
      ▌ [x] claude   personal   alice@example.com   browser
        [x] claude   work       alice@company.dev   browser
        [x] codex    codex      Codex · plus        device code

           IN PROXY ALREADY
            claude   alice@example.com              kept

     space toggle   a all   n none   enter continue   esc back
   ```

   - Lists the launcher's cla accounts (email from each account's config) and codex providers. All start ticked on the first run. `a` ticks all, `n` none.
   - Accounts already in the proxy (matched by email through the management API auth-file list) show as `kept` and are never logged in again or removed.
   - Logins: `cliproxyapi -claude-login` (browser) and `cliproxyapi -codex-device-login` (device code), pointed at the right config. Before each login, show "sign in as <email>". If the resulting email differs, warn and keep it.
   - Enter is blocked while nothing is ticked and the proxy has no account.
   - Checklist UI: fzf `--multi` when present, else a numbered list (`1 3`, `a`, `n`, Enter).
4. **Start.** Skip if the proxy already answers. Else `brew services start cliproxyapi` (or `start_cmd`), wait up to ~10 s for the proxy to answer, then apply failover mode and session affinity through the existing `apply` / `affinity` code paths.

**Run setup again** (`ai plugin setup cliproxy`) is the way to add accounts later.

### 6. Template and developer guide (user asked)

- `templates/plugin/` at the repo root (embedded in the TUI too): a commented bash `plugin` that implements `rows`, `actions`, `do`, `launch` with one example entry; a commented `setup` example (idempotent, `set -e`, clear messages); an `about`; a short `README.md`.
- `ai plugin new <name>` copies it into `<plugins>/<name>/` (same name checks, never overwrites), sets the exec bits, and prints the folder path. The plugin then shows up immediately. *My call:* bash command only; the TUI add form gets no "new" option.
- README "Writing a plugin" grows into a guide for developers and AI agents:
  - quick start (`ai plugin new`, edit, see it live)
  - the protocol table (existing)
  - `setup` and `about` contracts
  - rules: rows fast and never prompt, plain text, never print secrets, keep state in `AI_PLUGIN_DIR`, setup must be re-runnable and must not overwrite user config
  - how to test by hand (`AI_PLUGIN_DIR=… ./plugin rows`) and with a fake environment like `tests/cliproxy.sh`
  - how to publish (a git repo, `ai plugin add owner/repo`)
- AGENTS.md "Plugins" contract updated with `setup`, `about`, staging folders, and `ai plugin setup|new`.

### 7. Why not reuse existing logins (decided with user)

CLIProxyAPI has no importer, but an auth file can be built from the Claude Code / Codex login on disk. The user uses the same accounts both directly (original section) and through the proxy (plugin section). With a shared login, a token refresh on one side is expected to cancel the other side's copy, causing random logouts. This is not tested here, but it matches how these OAuth logins rotate refresh tokens. A fresh login per account avoids it completely. The cost is one sign-in per account, once.

## Facts (CLIProxyAPI source, main branch, read 2026-10-27)

- Login flags: `-claude-login`, `-codex-login`, `-codex-device-login`, global `-no-browser` (`cmd/server/main.go:138-148`). No Gemini login flag.
- Auth files: `POST/GET/DELETE /v0/management/auth-files` (`internal/api/server_management.go:180-188`).
- Config: default `config.yaml` in the working dir; `auth-dir` default `~/.cli-proxy-api`; client keys `access.api-keys`; management key `management.secret-key` (older `remote-management.secret-key` still read, `internal/config/config_load.go:244`), plaintext bcrypt-hashed on start and written back with comments kept (`:244-253`). Empty secret-key means the management API is off (404).
- The brew formula's config path is not documented in the repo.

## Left to the builder

- Where brew's `cliproxyapi` keeps `config.yaml` and how `brew services` passes it (check the formula); and how `-claude-login` must be pointed at the same config/auth dir.
- Whether the proxy picks up new auth files without a restart.
- Exact YAML edit for `secret-key` without a YAML tool (only replace an empty value; otherwise ask).
- Test layout: `tests/check.sh` for staging/approve/cancel/fail-rollback, `ai plugin new`, available listing; `tests/cliproxy.sh` for setup with fake `brew` and fake `cliproxyapi` on `PATH`; `cargo test` for the Rust side.
- Visual details of the screen within the mockups above.
