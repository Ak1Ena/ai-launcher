#!/usr/bin/env zsh
# Self-check: runs every feature against a throwaway fake HOME. Never touches your real configs.
#   zsh tests/check.zsh        exit 0 = all passed
emulate -L zsh
REPO=${0:A:h:h}
T=${$(mktemp -d):A}; trap 'rm -rf $T' EXIT
export HOME=$T/home CLA_ROOT=$T/home/acc CODEX_HOME=$T/home/.codex AI_PROVIDERS=$T/cfg/providers
mkdir -p $HOME $CLA_ROOT/{one,two}/usage $CODEX_HOME/sessions/2026/01/01 $HOME/.gemini/antigravity-cli $HOME/repo/{a,b}
source $REPO/ai.zsh
fails=0
ok() { if eval "$2"; then print "ok   $1"; else print "FAIL $1"; (( fails++ )); fi; }
plain() { sed $'s/\e\\[[0-9;]*m//g'; }
now=$EPOCHSECONDS

# ── providers ──
ok "first run seeds installed agents only" '_ai_names >/dev/null; [[ -e $AI_PROVIDERS ]]'
_ai_put zz "zz --flag" "Zed"; _ai_put yy "yy" "Why"
ok "add appends"                '[[ $(_ai_names | tail -1) == yy ]]'
_ai_put zz "zz --other" "Zed"
ok "edit keeps position"        '[[ $(_ai_names | tail -2 | head -1) == zz && $(_ai_cmd zz zz) == "zz --other" ]]'
_ai_put zz
ok "delete removes"             '! _ai_names | grep -qx zz'
ok "unknown provider -> default command" '[[ $(_ai_cmd nope nope) == nope ]]'
ok "command keeps quoted words" '[[ $(_ai_exec "/bin/echo \"a b\"" c) == "a b c" ]]'

# ── usage rows ──
echo '{"oauthAccount":{"emailAddress":"one@example.com"}}' > $CLA_ROOT/one/.claude.json
echo "{\"saved_at\":$now,\"rate_limits\":{\"five_hour\":{\"used_percentage\":34,\"resets_at\":$((now+3600))},\"seven_day\":{\"used_percentage\":95,\"resets_at\":$((now+86400))}}}" > $CLA_ROOT/one/usage/usage.json
echo "{\"saved_at\":$((now-7200)),\"rate_limits\":{\"five_hour\":{\"used_percentage\":80,\"resets_at\":$((now-60))}}}" > $CLA_ROOT/two/usage/usage.json
rows=$(_ai_claude_rows | cut -f3 | plain)
ok "claude row shows usage"     '[[ $rows == *"one@example.com"*"34%"*"95%"* ]]'
ok "passed reset shows 0%"      '[[ ${${(f)rows}[2]} == *" 0%"* ]]'
ok "old data shows its age"     '[[ ${${(f)rows}[2]} == *"2h old"* ]]'
ok "no login shows label"       '[[ ${${(f)rows}[2]} == *"not logged in"* ]]'
echo "{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":41.0,\"window_minutes\":300,\"resets_at\":$((now+60))},\"plan_type\":\"plus\"}}}" > $CODEX_HOME/sessions/2026/01/01/r.jsonl
ok "codex row reads session log" 'commands[codex]=/usr/bin/true; [[ $(_ai_codex_rows | cut -f3 | plain) == *"plus plan"*"5h"*"41%"* ]]'

# ── shared folder trust ──
echo '{"projects":{}}' > $HOME/.claude.json
printf '[projects."%s"]\ntrust_level = "trusted"\n' $HOME/repo/a > $CODEX_HOME/config.toml
echo "{\"$HOME/repo/a\": \"DO_NOT_TRUST\"}" > $HOME/.gemini/trustedFolders.json
echo '{"trustedWorkspaces":[]}' > $HOME/.gemini/antigravity-cli/settings.json
_ai_trust_share $HOME/repo/a; _ai_trust_share $HOME/repo/a; _ai_trust_share $HOME/repo/b
ok "trust spreads to claude (all accounts)" '_ai_trust_claude_has $HOME/repo/a && jq -e ".projects[\"$HOME/repo/a\"].hasTrustDialogAccepted" $CLA_ROOT/one/.claude.json >/dev/null'
ok "trust spreads to agy once"  '[[ $(jq ".trustedWorkspaces | length" $HOME/.gemini/antigravity-cli/settings.json) == 1 ]]'
ok "explicit untrust is kept"   '[[ $(jq -r ".[\"$HOME/repo/a\"]" $HOME/.gemini/trustedFolders.json) == DO_NOT_TRUST ]]'
ok "untrusted folder stays untrusted" '! _ai_trust_claude_has $HOME/repo/b && ! _ai_trust_codex_has $HOME/repo/b'
_cla_trust_sync $CLA_ROOT/one
ok "account sync keeps other fields" '[[ $(jq -r .oauthAccount.emailAddress $CLA_ROOT/one/.claude.json) == one@example.com ]]'

# ── status line script ──
echo '{"rate_limits":{"five_hour":{"used_percentage":5}},"x":"next"}' |
  CLAUDE_CONFIG_DIR=$T/sl bash $REPO/scripts/statusline-usage.sh jq -r .x > $T/sl.out
ok "status line saves usage"    '[[ $(jq .rate_limits.five_hour.used_percentage $T/sl/usage/usage.json) == 5 ]]'
ok "status line pipes to next"  '[[ $(<$T/sl.out) == next ]]'

print; (( fails )) && { print "$fails failed"; exit 1; } || print "all passed"
