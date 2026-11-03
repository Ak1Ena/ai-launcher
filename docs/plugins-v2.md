# Plugins v2 proposal

Status: manifest, order and `wrap` implemented on `feature/plugin-wrap`. Subdirectory installs and named slots below remain future work. Today's protocol (`rows`, `actions`, `menu`, `do`, `launch`) keeps working unchanged.

## Goal

Let other developers extend ai-launcher without forking it, starting with a headroom plugin that compresses context in front of any Claude entry, including cliproxy. Plugins **add** and **wrap**. They never take over the launcher's own data.

## Scope for external plugins

| Plugins may | How |
| --- | --- |
| Show info | `rows`, `menu`, `about` (exists) |
| Add entries | `launch` (exists) |
| Act on themselves | `do` (exists): start/stop their own service, sign in |
| Change one session | `wrap` (new): env, args, Claude settings, `--plugin-dir`, for that launch only |
| Install their own tool | reviewed `setup` (exists) |
| Replace a decision | named slots (later, one at a time when a real plugin needs one) |

Plugins may never:
- write the providers file, failover files, `.default`, other plugins' folders, `~/.claude`, `~/.codex`, `~/.gemini`, or shell rc files
- read other plugins' folders or account login folders
- use the network in `rows` or `wrap` (only in `setup`, `do` and their own `launch` or service)
- prompt in `rows`, `actions` or `wrap`
- take part in failover, WRR, limited marks or handoff

Bash cannot sandbox, so these rules are enforced by review for plugins in our plugins repo. Any other repo is "trust it like any script you run", and `ai plugin add` says so.

## Manifest

An optional `manifest` file in the plugin folder, with `key=value` lines read like cliproxy's config (no jq, never sourced):

```
api=1
hooks=wrap
after=
before=
team=0
```

- `api`: protocol version. No manifest means `api=0`: today's protocol, never called for `wrap`.
- `hooks`: the new hooks this plugin answers, comma-separated. The launcher calls only these, so it never spawns a plugin just to find out.
- `after` / `before`: plugin names this one should run after or before when both are on. They are hints only.
- `team`: `1` lets `wrap` run in team mode (`cla-team`) too. Missing or `0` means the plugin is skipped there. The plugin decides, because only it knows whether its change is safe for several agents sharing one session.

## Order

1. Start from plugin name order (bytewise, `LC_ALL=C`), not install order. Only enabled, runnable plugins participate.
2. Apply comma-separated `after` / `before` hints with a stable topological sort. Missing or disabled names are ignored. A cycle discards all hints, keeps name order and prints one warning.
3. `<providers dir>/plugins.order` (one plugin name per line), if present, wins over both. Named enabled plugins come first in the saved order; remaining plugins follow the hinted order. `ai plugin order a b c` writes it after validating every name; no args prints the effective order.

`wrap` runs in this order, and each layer sees the base URL produced by earlier layers. Therefore the **last** URL-changing plugin is closest to the agent: with `headroom, logger` the traffic is `claude → logger → headroom → upstream`. This follows the later-wins rule for env/settings rather than reversing hook execution.

## `wrap`

### When it runs

Only when an enabled plugin declares `wrap`, the launcher puts a shim folder first on `PATH` with `claude`, `codex` and `gemini` shims and sets `AI_WRAP_ENTRY` / `AI_WRAP_TEAM`. With no declarations it leaves PATH and those variables alone. A cla account, provider, or plugin entry that invokes an agent by name (such as cliproxy) reaches the shim. Absolute agent paths, shell functions and commands that replace PATH bypass it; PATH interposition cannot intercept those. Authentication and setup commands are not wrapped. Both front ends launch sessions through bash, including the TUI's `tui/src/pane.rs`.

The shim runs each `wrap` plugin, applies what they print, then runs the real binary (found by searching `PATH` without the shim folder). Hooks and the agent run with that cleaned `PATH`, so a `claude` started inside the session or by a hook is not wrapped again.

### Call

```
plugin wrap <agent> [agent args...]
```

Environment: `AI_PLUGIN_DIR`, `AI_WRAP_AGENT` (`claude|codex|gemini`), `AI_WRAP_ENTRY` (`claude/<account>`, a provider name, or `plugin:<name>/<id>`), `AI_WRAP_TEAM` (`1` in team mode, only reached when the manifest has `team=1`), and `AI_WRAP_BASE_URL`, which holds the base URL the agent would use right now after every earlier layer (empty means the agent's default). No tty, 10 s timeout. A failure, timeout or bad line skips that plugin with one warning, and the launch goes on.

### Output

One instruction per line. The plugin never runs the agent itself.

```
env<TAB>NAME<TAB>value          set an env var (claude: also written into the merged settings env block)
unset<TAB>NAME                  remove an env var
arg<TAB>value                   append one argument
settings<TAB>/path/file.json    claude only: a settings fragment to merge
plugin-dir<TAB>/path            claude only: adds --plugin-dir
```

### Claude settings are merged by the launcher

Tested with Claude Code 2.1.312: when `--settings` is given twice, **only the last file counts** and the earlier one is dropped, not merged. cliproxy already passes its own `--settings`, so a plugin adding a second one would silently undo it.

So the shim collects settings sources in order (existing `--settings path`, `--settings=path` or inline JSON, such as cliproxy's, then each plugin's `settings` fragments). It deep-merges objects with jq; arrays concatenate so hook/command lists stack, and later scalar values win. All `env` / `unset` instructions form the final settings env overlay in hook order (`unset` also removes a value inherited from settings). The result is one temp file (mode 600, removed when the agent exits) and a single `--settings` flag. Original files are never edited. Without jq, `settings` lines are skipped with one warning per plugin, existing `--settings` args are preserved, and `env` / `arg` / `plugin-dir` still apply.

### Single-owner keys

Keys that hold one value, such as `statusLine`, `outputStyle` and `theme`, can only have one owner. When two plugins set the same key, the later one in order wins, but the shim asks first, once per key and pair of plugins:

```
statusLine is set by cliproxy and by fancy-status. Use fancy-status's? [Y/n]
```

The answer is saved in `<providers dir>/plugins.owners` (`key<TAB>chosen-plugin<TAB>earlier-plugin<TAB>later-plugin`). The extra pair columns make both accepted and declined choices durable while ensuring a different competing pair asks again. Settings supplied by a `plugin:<name>/<id>` entry count as that plugin's settings; ordinary provider/account settings have no plugin owner. The shim asks on the launch's own terminal (stdin and stderr, never `/dev/tty`, the same as the pickers). Plugins still never prompt. With no terminal, or with `AI_YES=1`, the later one wins and a one-line warning is printed. `ai plugin owners` lists the saved choices, and `ai plugin owners reset` clears them.

Keys Claude Code already merges, such as plugin dirs, hooks and commands, stack without asking.

## Example: headroom over cliproxy

```
user picks "cliproxy · all accounts"
  cliproxy launch     -> runs: claude --settings cliproxy.json  (base URL :8317, key)
  shim                -> AI_WRAP_BASE_URL=http://127.0.0.1:8317
    headroom wrap     -> ensures "headroom proxy --port 8788 --anthropic-api-url http://127.0.0.1:8317" is running
                         prints: env  ANTHROPIC_BASE_URL  http://127.0.0.1:8788
  shim                -> merges cliproxy.json + base URL :8788 into one settings file
  real claude         -> headroom :8788 -> cliproxy :8317 -> accounts
```

The same plugin over a cla account sees an empty base URL, starts headroom with its default upstream (Anthropic), and the subscription login passes through. Headroom never reads cliproxy's files.

Tested with headroom's proxy against a fake upstream: both `Authorization: Bearer …` (cliproxy's key) and `x-api-key` reach the upstream unchanged.

Headroom keeps one proxy process per upstream (port picked from the upstream URL), so two sessions on different upstreams never restart each other's proxy. `do stop` ends them.

## Other agents

| Agent | Per-session hooks available |
| --- | --- |
| claude | env, args, merged `--settings`, `--plugin-dir` |
| codex | env (`OPENAI_BASE_URL`, …), args (`-c key=value` overrides `~/.codex/config.toml` for one run) |
| gemini | env, args. `GEMINI_CLI_SYSTEM_SETTINGS_PATH` exists but replaces the system settings file, so v1 does not use it |

`settings` and `plugin-dir` lines are ignored for codex and gemini in v1.

## Plugins repo

`Ak1Ena/ai-launcher-plugins`, one folder per plugin (`headroom/`, later others), plus starter templates in bash, Python and Node. Plugins can be written in any language, since a plugin is just an executable. The README repeats the scope rules above, and PRs are reviewed against them.

Install: `ai plugin add Ak1Ena/ai-launcher-plugins/headroom`. This needs `add` to accept `owner/repo/<folder>` (sparse clone, then use that folder), in both `lib/plugins.sh` and `tui/src/plugins.rs`.

## Bash and TUI

The bash tool and the TUI share `lib/shim/{claude,codex,gemini}`, `lib/wrap.sh` and `lib/plugins.sh`. The standalone TUI embeds these scripts (and their `lib/core.sh` helpers) and materializes them under its failover state directory only when needed. It only checks whether an enabled plugin declares `hooks=...,wrap,...`, prepends the shim to PATH and supplies launch metadata/config paths. It does **not** interpret API versions, team opt-in, dependency hints, order, output or owner policy: the shim does that. The TUI's team flag is currently false because it has no team launch action; bash `cla-team` sets it to 1. Manifest files live inside each plugin; `plugins.order` and `plugins.owners` live beside providers. No columns are added to existing provider/failover formats. Future `add` subdirectory parsing must still match in bash and Rust.

## Work plan

| Step | Branch | Release |
| --- | --- | --- |
| 1. This proposal, reviewed | `docs/plugins-v2` | none |
| 2. Manifest, order, shim, `wrap` (bash + TUI), tests in `tests/check.sh` | `feature/plugin-wrap` | minor |
| 3. `add owner/repo/<folder>` | `feature/plugin-add-subdir` | minor |
| 4. Plugins repo with `headroom/` and templates | new repo | (own) |
| 5. On demand: `before` / `after` hooks, first slot | `feature/…` | minor |

## Decided

- Single-owner keys: the later plugin in order wins, after asking the user once per key and pair (saved in `plugins.owners`).
- Team mode: each plugin opts in with `team=1` in its manifest. The default is off.

## Open questions

1. Windows: the shim works through Git Bash, but it is not yet tested on a real Windows machine.
