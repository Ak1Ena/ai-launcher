#!/usr/bin/env bash
# Self-check: runs every feature against a throwaway fake HOME. Never touches your real configs.
#   bash tests/check.sh        exit 0 = all passed   (also run it with /bin/bash on macOS: bash 3.2)
REPO=$(cd "$(dirname "$0")/.." && pwd)
T=$(cd "$(mktemp -d)" && pwd -P); trap 'rm -rf "$T"' EXIT   # resolved path: agents store getcwd()
export HOME=$T/home CLA_ROOT=$T/home/acc CODEX_HOME=$T/home/.codex AI_PROVIDERS=$T/cfg/providers
mkdir -p "$HOME" "$CLA_ROOT/one/usage" "$CLA_ROOT/two/usage" "$CODEX_HOME/sessions/2026/01/01" \
  "$HOME/.gemini/antigravity-cli" "$HOME/repo/a" "$HOME/repo/b"
. "$REPO/lib/core.sh"; . "$REPO/lib/trust.sh"; for f in "$REPO"/plugins/*.sh; do . "$f"; done
fails=0
ok() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }
plain() { sed $'s/\033\\[[0-9;]*m//g'; }
line() { sed -n "${1}p"; }
now=$(date +%s)

# ── providers ──
ok "first run creates the providers file" '_ai_names >/dev/null; [ -e "$AI_PROVIDERS" ]'
_ai_put zz "zz --flag" "Zed"; _ai_put yy "yy" "Why"
ok "add appends"                '[ "$(_ai_names | tail -1)" = yy ]'
_ai_put zz "zz --other" "Zed"
ok "edit keeps position"        '[ "$(_ai_names | tail -2 | head -1)" = zz ] && [ "$(_ai_cmd zz zz)" = "zz --other" ]'
_ai_put zz
ok "delete removes"             '! _ai_names | grep -qx zz'
ok "unknown provider -> default command" '[ "$(_ai_cmd nope nope)" = nope ]'
ok "command keeps quoted words" '[ "$(_ai_exec "printf \"%s|\" \"a b\"" c)" = "a b|c|" ]'
ok "command can set env vars"   '[ "$(_ai_exec "FOO=1 bash -c \"echo \\\$FOO\"")" = 1 ]'

# ── rows ──
echo '{"oauthAccount":{"emailAddress":"one@example.com"}}' > "$CLA_ROOT/one/.claude.json"
echo "{\"saved_at\":$now,\"rate_limits\":{\"five_hour\":{\"used_percentage\":34,\"resets_at\":$((now+3600))},\"seven_day\":{\"used_percentage\":95,\"resets_at\":$((now+86400))}}}" > "$CLA_ROOT/one/usage/usage.json"
echo "{\"saved_at\":$((now-7200)),\"rate_limits\":{\"five_hour\":{\"used_percentage\":80,\"resets_at\":$((now-60))}}}" > "$CLA_ROOT/two/usage/usage.json"
rows=$(_ai_claude_rows | cut -f3 | plain)
ok "claude row shows usage"     'case $(echo "$rows" | line 1) in *one@example.com*34%*95%*) true;; *) false;; esac'
ok "passed reset shows 0%"      'case $(echo "$rows" | line 2) in *" 0%"*) true;; *) false;; esac'
ok "old data shows its age"     'case $(echo "$rows" | line 2) in *"2h old"*) true;; *) false;; esac'
ok "no login shows label"       'case $(echo "$rows" | line 2) in *"not logged in"*) true;; *) false;; esac'
ok "columns line up"            '[ "$(echo "$rows" | line 1 | cut -c1-40 | wc -m)" = "$(echo "$rows" | line 2 | cut -c1-40 | wc -m)" ]'
echo "{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":41.0,\"window_minutes\":300,\"resets_at\":$((now+60))},\"plan_type\":\"plus\"}}}" > "$CODEX_HOME/sessions/2026/01/01/r.jsonl"
codex() { :; }; export -f codex 2>/dev/null
ok "codex row reads session log" 'case $(_ai_codex_rows | cut -f3 | plain) in *"plus plan"*5h*41%*) true;; *) false;; esac'
unset -f codex
printf 'claude\tclaude\tClaude Code\nfoo\tfoo --x\tFoo Tool\n' > "$AI_PROVIDERS"
rows=$(_ai_rows | cut -f1,2)
ok "rows keep providers order"  '[ "$(echo "$rows" | cut -f1 | uniq | tr "\n" " ")" = "claude foo " ]'
ok "plain provider row"         '[ "$(echo "$rows" | tail -1)" = "foo${TAB}foo" ]'

# ── shared folder trust ──
echo '{"projects":{}}' > "$HOME/.claude.json"
printf '[projects."%s"]\ntrust_level = "trusted"\n' "$HOME/repo/a" > "$CODEX_HOME/config.toml"
echo "{\"$HOME/repo/a\": \"DO_NOT_TRUST\"}" > "$HOME/.gemini/trustedFolders.json"
echo '{"trustedWorkspaces":[]}' > "$HOME/.gemini/antigravity-cli/settings.json"
_ai_trust_share "$HOME/repo/a"; _ai_trust_share "$HOME/repo/a"; _ai_trust_share "$HOME/repo/b"
ok "trust spreads to claude (all accounts)" '_ai_trust_claude_has "$HOME/repo/a" && jq -e ".projects[\"$HOME/repo/a\"].hasTrustDialogAccepted" "$CLA_ROOT/one/.claude.json" >/dev/null'
ok "trust spreads to agy once"  '[ "$(jq ".trustedWorkspaces | length" "$HOME/.gemini/antigravity-cli/settings.json")" = 1 ]'
ok "explicit untrust is kept"   '[ "$(jq -r ".[\"$HOME/repo/a\"]" "$HOME/.gemini/trustedFolders.json")" = DO_NOT_TRUST ]'
ok "untrusted folder stays untrusted" '! _ai_trust_claude_has "$HOME/repo/b" && ! _ai_trust_codex_has "$HOME/repo/b"'
echo '{"projects":{}, "oauthAccount":{"emailAddress":"two@example.com"}}' > "$CLA_ROOT/two/.claude.json"; chmod 600 "$CLA_ROOT/two/.claude.json"
_cla_trust_sync "$CLA_ROOT/two"
ok "account sync copies trust"  'jq -e ".projects[\"$HOME/repo/a\"].hasTrustDialogAccepted" "$CLA_ROOT/two/.claude.json" >/dev/null'
ok "account sync keeps other fields" '[ "$(jq -r .oauthAccount.emailAddress "$CLA_ROOT/two/.claude.json")" = two@example.com ]'
ok "account sync keeps permissions" '[ "$(ls -l "$CLA_ROOT/two/.claude.json" | cut -c1-10)" = "-rw-------" ]'

# ── Windows (Git Bash), faked with uname / cygpath functions ──
mkdir -p "$HOME/repo/w"
printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$HOME/repo/w" >> "$CODEX_HOME/config.toml"
ok "windows: trust sharing is off" '( uname() { echo MINGW64_NT-10.0; }; _ai_trust_share "$HOME/repo/w"; ! _ai_trust_claude_has "$HOME/repo/w" )'
ok "windows: native paths for agents" '( cygpath() { echo "C:/fake$2"; }; [ "$(native_path /x/y)" = C:/fake/x/y ] )'
ok "elsewhere: paths unchanged" '[ "$(native_path /x/y)" = /x/y ]'
ok "scripts keep LF endings"    '! grep -lq $'"'"'\r'"'"' "$REPO"/bin/ai "$REPO"/lib/*.sh "$REPO"/plugins/*.sh'
ok ".cmd files use CRLF"        'grep -q $'"'"'\r$'"'"' "$REPO/bin/ai.cmd"'

# ── status line script ──
echo '{"rate_limits":{"five_hour":{"used_percentage":5}},"x":"next"}' |
  CLAUDE_CONFIG_DIR=$T/sl bash "$REPO/scripts/statusline-usage.sh" jq -r .x > "$T/sl.out"
ok "status line saves usage"    '[ "$(jq .rate_limits.five_hour.used_percentage "$T/sl/usage/usage.json")" = 5 ]'
ok "status line pipes to next"  '[ "$(cat "$T/sl.out")" = next ]'

# ── entry point ──
ok "ai help runs"               '"$REPO/bin/ai" help | grep -q "ai add"'
ok "cla link runs cla"          '"$REPO/bin/cla" nope 2>&1 | grep -q "no account .nope."'

echo; if [ $fails -gt 0 ]; then echo "$fails failed"; exit 1; else echo "all passed"; fi
