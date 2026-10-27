#!/usr/bin/env bash
# Self-check: runs every feature against a throwaway fake HOME. Never touches your real configs.
#   bash tests/check.sh        exit 0 = all passed   (also run it with /bin/bash on macOS: bash 3.2)
REPO=$(cd "$(dirname "$0")/.." && pwd)
T=$(cd "$(mktemp -d)" && pwd -P); trap 'rm -rf "$T"' EXIT   # resolved path: agents store getcwd()
export HOME=$T/home CLA_ROOT=$T/home/acc CODEX_HOME=$T/home/.codex AI_PROVIDERS=$T/cfg/providers AI_ACCOUNTS=$T/home/.ai-accounts
mkdir -p "$HOME" "$CLA_ROOT/one/usage" "$CLA_ROOT/two/usage" "$CODEX_HOME/sessions/2026/01/01" \
  "$HOME/.gemini/antigravity-cli" "$HOME/repo/a" "$HOME/repo/b"
. "$REPO/lib/core.sh"; . "$REPO/lib/trust.sh"; . "$REPO/lib/failover.sh"; . "$REPO/lib/plugins.sh"; for f in "$REPO"/plugins/*.sh; do . "$f"; done
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
_ai_put codex codex "Codex CLI"
ok "codex row reads session log" 'case $(_ai_codex_rows | cut -f3 | plain) in *"Codex CLI · plus"*5h*41%*) true;; *) false;; esac'
unset -f codex
printf 'claude\tclaude\tClaude Code\nfoo\tfoo --x\tFoo Tool\n' > "$AI_PROVIDERS"
rows=$(_ai_rows | cut -f1,2)
ok "rows keep providers order"  '[ "$(echo "$rows" | cut -f1 | uniq | tr "\n" " ")" = "claude foo " ]'
ok "plain provider row"         '[ "$(echo "$rows" | tail -1)" = "foo${TAB}foo" ]'

# ── several codex / gemini logins through ai add ──
mkdir -p "$T/bin"; printf '#!/bin/sh\nprintf %%s "$CODEX_HOME"\n' > "$T/bin/codex"; chmod +x "$T/bin/codex"  # stub: never start the real agent
printf 'codex\tcodex\tCodex CLI\n' > "$AI_PROVIDERS"
ok "first codex keeps the main login" '[ -z "$(_ai_account_dir codex)" ]'
_ai_add work "FOO=1 $T/bin/codex --x" "Codex work" >/dev/null
ok "2nd codex gets its own login" '[ "$(_ai_agent work)" = codex ] && [ "$(_ai_account_dir work)" = "$AI_ACCOUNTS/codex/work" ]'
mkdir -p "$AI_ACCOUNTS/codex/work/sessions/2026/01/02"
echo "{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":7.0,\"window_minutes\":300,\"resets_at\":$((now+60))}}}}" > "$AI_ACCOUNTS/codex/work/sessions/2026/01/02/r.jsonl"
rows=$( PATH=$T/bin:$PATH; _ai_rows )
ok "each login has its own usage row" '[ "$(echo "$rows" | cut -f2 | tr "\n" " ")" = "codex work " ] && case $(echo "$rows" | line 2 | cut -f3 | plain) in *5h*7%*) true;; *) false;; esac'
ok "launch points codex at its login" '[ "$(_ai_run work work)" = "$(native_path "$AI_ACCOUNTS/codex/work")" ]'
_ai_put codex "$T/bin/codex"
mkdir -p "$AI_ACCOUNTS/codex/zz"
ok "main codex drops an inherited ai-account CODEX_HOME" '[ -z "$(export CODEX_HOME=$AI_ACCOUNTS/codex/zz; _ai_run codex codex)" ]'
ok "main codex keeps a CODEX_HOME set elsewhere" '[ "$(export CODEX_HOME=$T/elsewhere; _ai_run codex codex)" = "$T/elsewhere" ]'
ok "main login stays the default" '[ "$(_ai_run codex codex)" = "$CODEX_HOME" ]'
_ai_add other "aider" >/dev/null <<<""
ok "other CLIs never get a login folder" '[ ! -e "$AI_ACCOUNTS/aider" ]'
ok "names must be path-safe"    '! _ai_add .. codex x >/dev/null && ! _ai_add a/b codex x >/dev/null'
_ai_put work; _ai_add work codex "Codex work" >/dev/null
ok "re-adding a name keeps its login" '[ "$(_ai_account_dir work)" = "$AI_ACCOUNTS/codex/work" ]'
touch "$CODEX_HOME/auth.json"
ok "ai rm: n keeps the login folder" '! _ai_rm work <<<"n" >/dev/null 2>&1; [ -d "$AI_ACCOUNTS/codex/work" ]'
ok "ai rm: y deletes its login folder" '_ai_rm work <<<"y" >/dev/null 2>&1; [ ! -e "$AI_ACCOUNTS/codex/work" ] && [ -z "$(_ai_get work 1)" ]'
mkdir -p "$AI_ACCOUNTS/codex/codex"; touch "$AI_ACCOUNTS/codex/codex/auth.json"
ok "ai rm: main codex login untouched" '_ai_rm codex <<<"y" >/dev/null 2>&1; [ -z "$(_ai_get codex 1)" ] && [ -f "$CODEX_HOME/auth.json" ] && [ -f "$AI_ACCOUNTS/codex/codex/auth.json" ] && [ -d "$AI_ACCOUNTS/codex/zz" ]'

# ── shared folder trust ──
echo '{"projects":{}}' > "$HOME/.claude.json"
printf '[projects."%s"]\ntrust_level = "trusted"\n' "$HOME/repo/a" > "$CODEX_HOME/config.toml"
echo "{\"$HOME/repo/a\": \"DO_NOT_TRUST\"}" > "$HOME/.gemini/trustedFolders.json"
echo '{"trustedWorkspaces":[]}' > "$HOME/.gemini/antigravity-cli/settings.json"
_ai_trust_share "$HOME/repo/a"
ok "no trust sharing without the flag" '! _ai_trust_claude_has "$HOME/repo/a"'
ok "ai trust on makes the flag"  '[[ $(_ai_trust on) == *MCP* ]] && [ -e "${AI_PROVIDERS%/*}/trust.on" ] && _ai_trust status | grep -q ": on"'
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

# codex single-quoted header, json edit hardening
mkdir -p "$HOME/repo/q" "$HOME/repo/u"
printf "[projects.'%s']\ntrust_level = \"trusted\"\n[projects.'%s']\ntrust_level = \"untrusted\"\n" "$HOME/repo/q" "$HOME/repo/u" >> "$CODEX_HOME/config.toml"
_ai_trust_codex_add "$HOME/repo/q"; _ai_trust_codex_add "$HOME/repo/u"
ok "codex single-quoted header seen, no duplicate" '_ai_trust_codex_has "$HOME/repo/q" && [ "$(grep -c "repo/q" "$CODEX_HOME/config.toml")" = 1 ]'
ok "codex single-quoted untrusted kept" '! _ai_trust_codex_has "$HOME/repo/u" && [ "$(grep -c "repo/u" "$CODEX_HOME/config.toml")" = 1 ]'
J=$T/je.json; echo '{"a":1}' > "$J"; chmod 640 "$J"
_ai_json_edit "$J" '.a = 2'; _ai_json_edit "$J" '.a = 3'
ok "json edit applies, .bak made once" '[ "$(jq .a "$J")" = 3 ] && [ "$(jq .a "$J.ai-launcher.bak")" = 1 ]'
ok "json edit keeps mode"        '[ "$(ls -l "$J" | cut -c1-10)" = "-rw-r-----" ]'
echo '{bad' > "$T/bad.json"; _ai_json_edit "$T/bad.json" '.a = 1'
ok "bad json input left untouched" '[ "$(cat "$T/bad.json")" = "{bad" ] && [ ! -e "$T/bad.json.ai-launcher.bak" ]'
ln -s "$J" "$T/lnk.json"; _ai_json_edit "$T/lnk.json" '.a = 9'
ok "json edit writes through symlink" '[ -L "$T/lnk.json" ] && [ "$(jq .a "$J")" = 9 ]'
ok "no temp files left" '[ -z "$(ls "$T" | grep "^je\.json\." | grep -v "\.ai-launcher\.bak$")" ]'

# ── Windows (Git Bash), faked with uname / cygpath functions ──
mkdir -p "$HOME/repo/w"
printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$HOME/repo/w" >> "$CODEX_HOME/config.toml"
ok "windows: trust sharing is off" '( uname() { echo MINGW64_NT-10.0; }; _ai_trust_share "$HOME/repo/w"; ! _ai_trust_claude_has "$HOME/repo/w" )'
ok "windows: native paths for agents" '( cygpath() { echo "C:/fake$2"; }; [ "$(native_path /x/y)" = C:/fake/x/y ] )'
ok "elsewhere: paths unchanged" '[ "$(native_path /x/y)" = /x/y ]'
ok "scripts keep LF endings"    '! grep -lq $'"'"'\r'"'"' "$REPO"/bin/ai "$REPO"/lib/*.sh "$REPO"/plugins/*.sh'
ok ".cmd files use CRLF"        'grep -q $'"'"'\r$'"'"' "$REPO/bin/ai.cmd"'

# ── failover ──
FO=$T/fo; mkdir -p "$FO/bin" "$FO/proj/-slug" "$FO/state/limited/claude"; AI_FO_STATE=$FO/state; AI_FO_ORDER=$T/cfg/failover; rm -f "$AI_FO_ORDER" "$AI_FO_ORDER.on"
# stub claude: --help lists value flags; run as a provider command it logs its args and, unless resumed, "hits the limit"
cat > "$FO/bin/claude" <<'EOS'
#!/bin/sh
if [ "$1" = --help ]; then printf '  --model <model>   m\n  --allowedTools, --allowed-tools <tools...>   t\n  -p, --print   p\n'; exit 0; fi
echo "$CLAUDE_CONFIG_DIR|$*" >> "$FO_LOG"
case " $* " in *" --resume "*) ;; *) printf '{"session_id":"S1","transcript_path":"%s"}' "$FO_TR" > "$AI_FO_HANDOFF" ;; esac
EOS
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "$FO_LOG.codex"\n' > "$FO/bin/codex"; chmod +x "$FO/bin/claude" "$FO/bin/codex"
printf '%s\n' '{"type":"user","message":{"content":"hello world"}}' '{"type":"assistant","message":{"content":[{"type":"text","text":"half done"}]}}' > "$FO/proj/-slug/S1.jsonl"
printf 'claude\t%s\tClaude\ncl2\t%s\tClaude 2\ncx\t%s\tCodex\nother\taider\tAider\n' "$FO/bin/claude" "$FO/bin/claude" "$FO/bin/codex" > "$AI_PROVIDERS"
members() { _fo_list | cut -f1 | tr '\n' ' '; }
stateof() { _fo_list | awk -F'\t' -v m="$1" '$1 == m { print $3 }'; }
touch_limit() { mkdir -p "$(dirname "$AI_FO_STATE/limited/$1")"; touch "$AI_FO_STATE/limited/$1"; }
ok "failover is off by default"   '[ "$(_fo_state)" = off ]'
_ai_failover on >/dev/null 2>&1
ok "failover on toggles"          '_fo_on && [ "$(_fo_state)" = on ]'
_ai_failover off >/dev/null 2>&1
ok "failover off toggles"         '! _fo_on && [ "$(_fo_state)" = off ]'
ok "members: accounts + claude/codex providers only" '[ "$(members)" = "claude/one claude/two cl2 cx " ]'
ok "new members default on"       '[ "$(_fo_list | cut -f3 | sort -u)" = on ]'
ok "claude kinds before codex"    '[ "$(_fo_list | cut -f2 | tr "\n" " ")" = "claude claude claude codex " ]'
_fo_list | _fo_move cx up | _fo_move cx up | _fo_save
ok "move up + save persists"      '[ "$(members)" = "claude/one cx claude/two cl2 " ]'
_fo_list | _fo_move cx down | _fo_save
ok "move down persists"           '[ "$(members)" = "claude/one claude/two cx cl2 " ]'
_fo_list | _fo_set cl2 | _fo_save
ok "set toggles off"              '[ "$(stateof cl2)" = off ]'
_ai_failover order cl2 claude/two >/dev/null 2>&1
ok "order puts names first"       '[ "$(members)" = "cl2 claude/two claude/one cx " ]'
ok "order keeps off state"        '[ "$(stateof cl2)" = off ]'
_fo_list | _fo_set cl2 on | _fo_save
touch_limit cx
ok "limited member reports seconds" '[ "$(_fo_limited cx)" -gt 0 ]'
ok "free member is not limited"   '! _fo_limited claude/one'
ok "next skips tried and limited" '[ "$(_fo_next " claude/two ")" = cl2 ] && [ "$(_fo_next " cl2 claude/two claude/one ")" != cx ]'
_fo_list | _fo_set claude/one off | _fo_save
ok "next skips off members"       '! _fo_next " cl2 claude/two cx "'
ok "clear frees limited members"  '_ai_failover clear >/dev/null 2>&1; ! _fo_limited cx'
touch_limit cx
ok "cooldown expires"             '( AI_FO_COOLDOWN=0; ! _fo_limited cx )'
_ai_failover clear >/dev/null 2>&1
rm -f "$AI_FO_ORDER"
ok "split: keep flags and prompt" '( PATH=$FO/bin:$PATH; _fo_split -p "x" --model m --allowedTools Bash --resume id; [ "${FO_KEEP[*]}" = "-p --model m --allowedTools Bash" ] && [ "$FO_PROMPT" = x ] )'
# end to end: claude/one hits the limit, claude/two resumes S1
export FO_LOG=$FO/log FO_TR=$FO/proj/-slug/S1.jsonl; : > "$FO_LOG"
_ai_failover on >/dev/null 2>&1; _ai_failover order claude/one claude/two cx >/dev/null 2>&1
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi --model m ) 2>/dev/null <<<"y"
ok "run: next account resumes the session" 'grep -q "^$CLA_ROOT/two|.*--model m --resume S1" "$FO_LOG"'
ok "run: transcript copied to its config dir" '[ -f "$CLA_ROOT/two/projects/-slug/S1.jsonl" ]'
ok "run: limit is remembered"     '_fo_limited claude/one >/dev/null'
# claude/two off: the next stop is the codex provider, which gets a handoff with the transcript
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; rm -f "$FO_LOG.codex"
_fo_list | _fo_set claude/two off | _fo_save
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) 2>"$FO/err" <<<"y"
ok "run: codex gets a handoff"    'grep -q "hello world" "$FO_LOG.codex" && grep -q "half done" "$FO_LOG.codex"'
ok "run: codex next warns it goes to OpenAI" 'grep -q "sends the conversation to OpenAI (Codex)" "$FO/err"'
# enter at the switch card: default No, the next member never starts
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; rm -f "$FO_LOG.codex"
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) 2>/dev/null <<<""; noRC=$?
ok "run: enter at the switch card does not move" '[ $noRC != 0 ] && [ ! -e "$FO_LOG.codex" ] && [ "$(wc -l < "$FO_LOG" | tr -d " ")" = 1 ]'
# started inside a cla session: CLAUDE_CONFIG_DIR under CLA_ROOT is not main
mkdir -p "$T/elsewhere"; : > "$FO_LOG"
( CLAUDE_CONFIG_DIR=$CLA_ROOT/one; PATH=$FO/bin:$PATH; _fo_run cl2 ) </dev/null >/dev/null 2>&1
( CLAUDE_CONFIG_DIR=$T/elsewhere; PATH=$FO/bin:$PATH; _fo_run cl2 ) </dev/null >/dev/null 2>&1
ok "run: cla CLAUDE_CONFIG_DIR is not main" '[ "$(cut -d"|" -f1 "$FO_LOG" | tr "\n" " ")" = " $T/elsewhere " ]'
_ai_failover clear >/dev/null 2>&1
ok "run: transcript copy keeps its mode" '( chmod 600 "$FO_TR"; rm -rf "$CLA_ROOT/two/projects"; _fo_list | _fo_set claude/two on | _fo_save; _ai_failover clear >/dev/null 2>&1; PATH=$FO/bin:$PATH; _fo_run claude/one -p hi 2>/dev/null <<<"y" >/dev/null; [ "$(ls -l "$CLA_ROOT/two/projects/-slug/S1.jsonl" | cut -c1-10)" = "-rw-------" ] )'
# q at the switch card: stops, next member never launched
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; _fo_list | _fo_set claude/two on | _fo_save
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) 2>/dev/null <<<"q"; stoprc=$?
ok "run: q at the switch card stops" '[ $stoprc != 0 ] && [ "$(wc -l < "$FO_LOG" | tr -d " ")" = 1 ] && ! grep -q "^$CLA_ROOT/two|" "$FO_LOG"'
# auto-switch: <order>.auto
ok "failover auto on writes the file" '_ai_failover auto on >/dev/null 2>&1; [ -e "$AI_FO_ORDER.auto" ] && _fo_auto'
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"
( AI_FO_WAIT=2; PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) </dev/null >/dev/null 2>&1
ok "auto: countdown moves with no input" 'grep -q "^$CLA_ROOT/two|.*--resume S1" "$FO_LOG"'
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"
( AI_FO_WAIT=2; PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) 2>/dev/null <<<"q"; stoprc=$?
ok "auto: q stops the countdown" '[ $stoprc != 0 ] && ! grep -q "^$CLA_ROOT/two|" "$FO_LOG"'
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; rm -f "$FO_LOG.codex"
_fo_list | _fo_set claude/two off | _fo_save
( AI_FO_WAIT=2; PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) </dev/null >/dev/null 2>"$FO/err"; cxrc=$?
ok "auto: codex next counts down too, with the OpenAI line" '[ -e "$FO_LOG.codex" ] && grep -q "sends the conversation to OpenAI (Codex)" "$FO/err" && grep -q "switching in" "$FO/err"'
_fo_list | _fo_set claude/two on | _fo_save; _ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"
# stdin stays open but silent: only the countdown running out can move it (y/N would wait for EOF at 4 s)
sleep 4 | { s=$(date +%s); ( AI_FO_WAIT=2; PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) >/dev/null 2>&1; echo $(( $(date +%s) - s )) > "$FO/took"; }
ok "auto: countdown runs out and moves" 'grep -q "^$CLA_ROOT/two|.*--resume S1" "$FO_LOG" && [ "$(cat "$FO/took")" -ge 2 ] && [ "$(cat "$FO/took")" -lt 4 ]'
_ai_failover clear >/dev/null 2>&1
ok "auto: wait counts down to go"   '[ "$(AI_FO_WAIT=1 _fo_wait 2>/dev/null </dev/null)" = go ]'
ok "auto: ls shows the state" '_fo_show 2>&1 | grep -q "auto-switch on"'
ok "failover auto off removes the file" '_ai_failover auto off >/dev/null 2>&1; [ ! -e "$AI_FO_ORDER.auto" ] && ! _fo_auto && _fo_show 2>&1 | grep -q "auto-switch off"'
mrows=$(_fo_rows)
ok "menu order: u 2 is up"        '[ "$(_ai_menu t "" order "$mrows" <<<"u 2" 2>/dev/null | head -1)" = up ]'
ok "menu order: x 1 is toggle"    '[ "$(_ai_menu t "" order "$mrows" <<<"x 1" 2>/dev/null | head -1)" = toggle ]'
ok "menu order: a is auto"        '[ "$(_ai_menu t "" order "$mrows" <<<"a" 2>/dev/null | head -1)" = auto ]'
ok "menu keys: a is still add"    '[ "$(_ai_menu t "" keys "$mrows" <<<"a" 2>/dev/null | head -1)" = add ]'
ok "fzf order: ctrl-a is auto"    '[ "$( fzf() { cat >/dev/null; printf "ctrl-a\nr\n"; }; _ai_fzf t "" order "$mrows" | head -1)" = auto ]'
# the priority screen: auto toggles <order>.auto, and its hint shows the state
FZ=$T/fz; rm -f "$AI_FO_ORDER.auto"
ok "order screen: auto toggles auto-switch" '( fzf() { cat >/dev/null; [ -e "$FZ" ] && return 1; : > "$FZ"; printf "ctrl-a\n%s\n" "$(_fo_rows | head -1)"; }; _fo_ui >/dev/null 2>&1 ); [ -e "$AI_FO_ORDER.auto" ]'
ok "order screen: hint shows auto-switch state" '( fzf() { cat >/dev/null; printf "%s\n" "$@" > "$FZ.args"; return 1; }; _fo_ui >/dev/null 2>&1 ); grep -q "ctrl-a auto-switch: on" "$FZ.args"'
rm -f "$AI_FO_ORDER.auto" "$FZ"
ok "menu keys: x 1 is delete"     '[ "$(_ai_menu t "" keys "$mrows" <<<"x 1" 2>/dev/null | head -1)" = delete ]'
ok "menu keys: u 2 fails"         '! _ai_menu t "" keys "$mrows" <<<"u 2" >/dev/null 2>&1'
# clear <member> never leaves limited/
mkdir -p "$AI_FO_STATE/limited/claude"; touch "$AI_FO_STATE/keep" "$AI_FO_STATE/limited/claude/one"
ok "clear rejects path tricks" '! _ai_failover clear ../.. >/dev/null 2>&1 && ! _ai_failover clear claude/../.. >/dev/null 2>&1 && ! _ai_failover clear "a\\b" >/dev/null 2>&1 && ! _ai_failover clear .x >/dev/null 2>&1 && [ -e "$AI_FO_STATE/keep" ] && [ -e "$AI_FO_STATE/limited/claude/one" ]'
ok "clear claude/<account> still works" '_ai_failover clear claude/one >/dev/null 2>&1 && [ ! -e "$AI_FO_STATE/limited/claude/one" ] && [ -d "$AI_FO_STATE/limited/claude" ]'
_ai_failover off >/dev/null 2>&1

# ── failover: screens ──
vis() { plain | wc -m | tr -d ' '; }
card=$(_fo_card "T" "short" $'\033[1mbold\033[0m ünï ✻ wide line here' 2>&1 >/dev/null | plain | sed '/^$/d')
ok "card: box lines share one width" '[ "$(echo "$card" | while IFS= read -r l; do printf "%s" "$l" | wc -m; done | sort -u | wc -l | tr -d " ")" = 1 ]'
ok "card: has top, body and bottom" '[ "$(echo "$card" | wc -l | tr -d " ")" = 4 ] && echo "$card" | line 1 | grep -q "^╭" && echo "$card" | line 4 | grep -q "^╰"'
ok "label: claude account"        '[ "$(_fo_label claude/x)" = "Claude account x" ]'
ok "label: provider with label"   '[ "$(_fo_label cl2)" = "Claude 2 (cl2)" ]'
ok "label: provider without label" '[ "$(_fo_label other2)" = other2 ]'
ok "wait: y moves"                '[ "$(_fo_wait 2>/dev/null <<<"y")" = go ] && [ "$(_fo_wait 2>/dev/null <<<"Y")" = go ]'
ok "wait: enter stops (default No)" '[ "$(_fo_wait 2>/dev/null <<<"")" = stop ]'
ok "wait: o is order"             '[ "$(_fo_wait 2>/dev/null <<<"o")" = order ]'
ok "wait: q is stop"              '[ "$(_fo_wait 2>/dev/null <<<"q")" = stop ]'
ok "wait: no input stops"         '[ "$(_fo_wait 2>/dev/null </dev/null)" = stop ]'
_ai_failover order cl2 claude/two claude/one cx >/dev/null 2>&1
shown=$(_fo_show 2>&1 | plain)
ok "show: lists members in order" '[ "$(echo "$shown" | grep -o "cl2\|claude/two\|claude/one\|cx" | tr "\n" " ")" = "cl2 claude/two claude/one cx " ]'
rm -f "$AI_FO_ORDER"

# ── status line script ──
echo '{"rate_limits":{"five_hour":{"used_percentage":5}},"x":"next"}' |
  CLAUDE_CONFIG_DIR=$T/sl bash "$REPO/scripts/statusline-usage.sh" jq -r .x > "$T/sl.out"
ok "status line saves usage"    '[ "$(jq .rate_limits.five_hour.used_percentage "$T/sl/usage/usage.json")" = 5 ]'
ok "status line pipes to next"  '[ "$(cat "$T/sl.out")" = next ]'

# ── entry point ──
ok "ai help runs"               '"$REPO/bin/ai" help | grep -q "ai add"'
ok "cla link runs cla"          '"$REPO/bin/cla" nope 2>&1 | grep -q "no account .nope."'
# ── cla add/rm/mv safety ──
ok "cla add rejects ../x"       '! "$REPO/bin/cla" add ../x >/dev/null 2>&1 && [ ! -e "$CLA_ROOT/../x" ]'
ok "cla add rejects .x"         '! "$REPO/bin/cla" add .x >/dev/null 2>&1 && [ ! -e "$CLA_ROOT/.x" ]'
mkdir -p "$CLA_ROOT/gone" "$T/clabin"; touch "$CLA_ROOT/gone/f"
printf '#!/bin/sh\nexit 0\n' > "$T/clabin/claude"; chmod +x "$T/clabin/claude"
printf '#!/bin/sh\necho called >> "%s/sec.log"\n' "$T" > "$T/clabin/security"; chmod +x "$T/clabin/security"
ok "cla rm: AI_YES=0 still asks" 'echo n | AI_YES=0 PATH=$T/clabin:$PATH "$REPO/bin/cla" rm gone 2>&1 | grep -q "\[y/N\]"; [ -d "$CLA_ROOT/gone" ]'
mkdir -p "$CLA_ROOT/b\\s"
ok "cla rm rejects a backslash name" '! AI_YES=1 PATH=$T/clabin:$PATH "$REPO/bin/cla" rm "b\\s" >/dev/null 2>&1 && [ -d "$CLA_ROOT/b\\s" ]'
rmdir "$CLA_ROOT/b\\s"
mkdir -p "$T/badbin"; printf '#!/bin/sh\nexit 1\n' > "$T/badbin/claude"; chmod +x "$T/badbin/claude"
ok "cla rm: failed logout warns and asks again" 'printf "y\nn\n" | PATH=$T/badbin:$PATH "$REPO/bin/cla" rm gone 2>&1 | grep -q "logout failed"; [ -d "$CLA_ROOT/gone" ]'
ok "cla rm: failed logout, y again deletes" 'printf "y\ny\n" | PATH=$T/badbin:$PATH "$REPO/bin/cla" rm gone >/dev/null 2>&1; [ ! -e "$CLA_ROOT/gone" ]'
mkdir -p "$CLA_ROOT/gone"; touch "$CLA_ROOT/gone/f"
ok "cla rm: failed logout, AI_YES=1 deletes" 'AI_YES=1 PATH=$T/badbin:$PATH "$REPO/bin/cla" rm gone </dev/null >/dev/null 2>&1; [ ! -e "$CLA_ROOT/gone" ]'
mkdir -p "$CLA_ROOT/gone"; touch "$CLA_ROOT/gone/f"
ok "cla rm cancels without y"   'echo n | PATH=$T/clabin:$PATH "$REPO/bin/cla" rm gone >/dev/null 2>&1; [ -d "$CLA_ROOT/gone" ]'
ok "cla rm with y deletes for good" 'echo y | PATH=$T/clabin:$PATH "$REPO/bin/cla" rm gone >/dev/null 2>&1; [ ! -e "$CLA_ROOT/gone" ] && [ ! -e "$CLA_ROOT/.trash" ]'
mkdir -p "$T/keepme"; touch "$T/keepme/f"; ln -s "$T/keepme" "$CLA_ROOT/lnk"
ok "cla rm of a linked account drops only the link" 'AI_YES=1 PATH=$T/clabin:$PATH "$REPO/bin/cla" rm lnk >/dev/null 2>&1; [ ! -L "$CLA_ROOT/lnk" ] && [ -f "$T/keepme/f" ]'
mkdir -p "$CLA_ROOT/my acct" "$CLA_ROOT/yes1"
ok "cla rm removes a folder with an old-style name" 'PATH=$T/clabin:$PATH AI_YES=1 "$REPO/bin/cla" rm "my acct" >/dev/null 2>&1; [ ! -d "$CLA_ROOT/my acct" ]'
ok "cla rm: AI_YES=1 skips the confirm" 'PATH=$T/clabin:$PATH AI_YES=1 "$REPO/bin/cla" rm yes1 </dev/null >/dev/null 2>&1; [ ! -d "$CLA_ROOT/yes1" ]'
mkdir -p "$CLA_ROOT/mva"
ok "cla mv never calls security" 'PATH=$T/clabin:$PATH "$REPO/bin/cla" mv mva mvb >/dev/null 2>&1; [ -d "$CLA_ROOT/mvb" ] && [ ! -e "$T/sec.log" ]'
rmdir "$CLA_ROOT/mvb"

# ── failover: review fixes ──
printf 'gone\toff\n' >> "$AI_FO_ORDER"; _fo_list | _fo_save
ok "save keeps lines of absent members"   'grep -q "^gone${TAB}off$" "$AI_FO_ORDER"'
: | _fo_save
ok "empty list never truncates the order" '[ -s "$AI_FO_ORDER" ]'
printf '%s\n' '{"type":"user","message":{"content":"short one"}}' > "$T/small.jsonl"; mkdir -p "$AI_FO_STATE"
ok "small handoff keeps its first line"   '[ "$(_fo_handoff "$T/small.jsonl" | head -1)" = "### user" ]'
ok "provider dir falls back to start dir" '[ "$(FO_BASE_CFG=/x/base _fo_claude_dir nosuch)" = /x/base ]'
ok "windows: failover stays off"          '( uname() { echo MINGW64_NT-10.0; }; : > "$AI_FO_ORDER.on"; ! _fo_on && ! _ai_failover on >/dev/null 2>&1 )'

# ── update notice ──
AI_HOME=$REPO; cur=$(sed -n 's/^version *= *"\(.*\)".*/\1/p' "$REPO/tui/Cargo.toml" | head -1)
uc=$(dirname "$AI_FO_STATE")/update-check; mkdir -p "$(dirname "$uc")"
mkdir -p "$T/nonpm"; npm() { echo "network used" > "$T/npm.called"; }   # a stub: any call is a test failure
setc() { printf '%s\t%s\n' "${2:-$now}" "$1" > "$uc"; }
setc 99.0.0;   ok "update: newer cached -> notice"       'case $(_ai_update_notice) in *"update 99.0.0 available"*) true;; *) false;; esac'
setc "$cur";   ok "update: equal -> hidden"              '[ -z "$(_ai_update_notice)" ]'
setc 0.0.1;    ok "update: older -> hidden"              '[ -z "$(_ai_update_notice)" ]'
setc 99.0.0-rc1; ok "update: prerelease of newer major shows" '[ -n "$(_ai_update_notice)" ]'
setc "$cur-rc1"; ok "update: prerelease below release hidden" '[ -z "$(_ai_update_notice)" ]'
setc garbage;  ok "update: garbage -> hidden"            '[ -z "$(_ai_update_notice)" ]'
setc 99.0.0;   ok "update: off switch"                   '[ -z "$(AI_LAUNCHER_NO_UPDATE_CHECK=1 _ai_update_notice)" ]'
ok "update: fresh cache never calls npm"                 '[ ! -e "$T/npm.called" ]'
setc 99.0.0 $((now - 90000)); _ai_update_notice >/dev/null; sleep 1
ok "update: stale cache shows nothing, refreshes in background" '[ -e "$T/npm.called" ] && [ -z "$(_ai_update_notice)" ]'
unset -f npm; rm -f "$uc"
ok "update: no cache, no npm -> silent"                  '[ -z "$(PATH=$T/nonpm:/usr/bin:/bin _ai_update_notice)" ] && [ ! -e "$uc" ]'

# ── default account ──
mkdir -p "$T/dbin"; printf '#!/bin/sh\necho "dflt $*"\n' > "$T/dbin/dflt"; chmod +x "$T/dbin/dflt"
_ai_put dflt "$T/dbin/dflt" Dflt; rm -f "$AI_FO_ORDER.default"
ok "default: unknown name refused"     '! _ai_default nosuch >/dev/null && ! _ai_default claude >/dev/null && ! _ai_default claude/.. >/dev/null && [ ! -e "$AI_FO_ORDER.default" ]'
_ai_default dflt >/dev/null
ok "default: saved beside failover"    '[ "$(cat "$AI_FO_ORDER.default")" = dflt ] && [ "$(_ai_default)" = dflt ]'
ok "default: ai starts it with args"   '[ "$(ai_main --x 2>&1)" = "dflt --x" ]'
ok "default: ai pick skips it"         '[ "$(echo q | PATH=/usr/bin:/bin ai_main pick 2>/dev/null)" != "dflt " ]'
ok "default: claude/<account> valid"   '_ai_default_ok claude/one'
_ai_put dflt; ok "default: gone -> picker" '[ "$(echo q | PATH=/usr/bin:/bin ai_main 2>/dev/null)" = "" ]'
_ai_default - >/dev/null; ok "default: cleared" '[ ! -e "$AI_FO_ORDER.default" ]'

# ── user plugins ──
export AI_LAUNCHER_NO_UPDATE_CHECK=1
PL=${AI_PROVIDERS%/*}/plugins
mkplug() { mkdir -p "$PL/$1"; printf '#!/bin/sh\n%s\n' "$2" > "$PL/$1/plugin"; chmod +x "$PL/$1/plugin"; case $1 in slow|off) ;; *) "$PL/$1/plugin" rows >/dev/null 2>&1 ;; esac; }  # warm: macOS scans a new script on first run
mkplug good 'case $1 in rows) printf "section\tGOOD STUFF\na\tAlpha\tready\n  \tdetail one\nb\tBeta\tidle\njunk\n" ;; do) echo "did $2"; echo oops >&2 ;; launch) shift; echo "launch $* in $AI_PLUGIN_DIR at $PWD" ;; esac'
mkplug slow "sleep 31$$"  # pid in the number: other test runs' sleeps cannot match
mkplug bad 'exit 1'
mkplug off "touch $T/off.ran; printf 'section\tOFF\n'"; : > "$PL/off/.off"
mkplug empty 'printf "section\tEMPTY\n"'
mkplug x.y 'printf "section\tXY\n"'; mkdir -p "$PL/nofile"
ok "plugins: names sorted, bad folders skipped" '[ "$(_ai_plugin_names | tr "\n" " ")" = "bad empty good off slow " ]'
s=$(date +%s); prow=$(_ai_rows); took=$(( $(date +%s) - s ))
ok "plugins: sections in name order" '[ "$(echo "$prow" | cut -f3 | plain | grep -o "bad: plugin failed\|EMPTY\|GOOD STUFF\|off · off\|slow: plugin failed" | tr "\n" "|")" = "bad: plugin failed|EMPTY|GOOD STUFF|off · off|slow: plugin failed|" ]'
ok "plugins: a hung plugin times out fast" '[ $took -lt 5 ]'
sleep 1
ok "plugins: timeout kills the child tree" '! ps -A -o command | grep -q "^sleep 31$$\$"'
ok "plugins: entries carry plugin:<name> and id" '[ "$(echo "$prow" | grep "^plugin:good" | cut -f2 | tr "\n" " ")" = "a b " ]'
ok "plugins: detail line under its entry, not selectable" '[ "$(echo "$prow" | grep -A1 "^plugin:good${TAB}a${TAB}" | line 2 | cut -f1,2)" = "$TAB" ] && echo "$prow" | grep -A1 "^plugin:good${TAB}a${TAB}" | line 2 | grep -q "detail one"'
ok "plugins: junk lines ignored" '! echo "$prow" | grep -q junk'
ok "plugins: zero entries says so" 'echo "$prow" | grep -A1 EMPTY | line 2 | plain | grep -q "no entries"'
ok "plugins: an off plugin is never run" '[ ! -e "$T/off.ran" ]'
FZ=$T/pfz; rm -f "$FZ"
pick() { fzf() { cat >/dev/null; [ -e "$FZ" ] && return 1; : > "$FZ"; printf "%s\n%s\n" "${KEY:-}" "$PICK"; }; cd "$HOME/repo/a"; ai_main "$@"; }
ok "plugins: picking an entry runs plugin launch" '[ "$(PICK="plugin:good${TAB}a${TAB}x" pick pick --y 2>&1)" = "launch a --y in $PL/good at $HOME/repo/a" ]'
rm -f "$FZ"
ok "plugins: header rows do nothing" '[ -z "$(PICK="${TAB}${TAB}GOOD STUFF" pick pick 2>&1)" ]'
rm -f "$FZ"
ok "plugins: an off plugin never launches" '[ "$(PICK="plugin:off${TAB}a${TAB}x" pick pick 2>&1 | grep -c launch)" = 0 ] && [ ! -e "$T/off.ran" ]'
cp "$AI_PROVIDERS" "$T/prov.bak"
for k in team edit delete; do rm -f "$FZ"
  ok "plugins: $k key on a plugin row does nothing" '[ "$(KEY=$k PICK="plugin:good${TAB}a${TAB}x" pick pick 2>&1 | grep -c launch)" = 0 ]'; done
rm -f "$FZ"; ok "plugins: header row + team key just redraws" '[ -z "$(KEY=team PICK="${TAB}${TAB}GOOD STUFF" pick pick 2>&1)" ]'
ok "plugins: providers file untouched by those keys" 'cmp -s "$AI_PROVIDERS" "$T/prov.bak" && ! grep -q "plugin:" "$AI_PROVIDERS"'
rm -f "$FZ"
mkplug multi 'printf "a\tAlpha\ts1\ts2\tS3\n  \td-a\nb d\tBad\tx\n  \td-dropped\nc\tCee\n"'
mrow=$(_ai_plugin_rows multi | plain)
ok "plugins: extra tab fields join into status" 'echo "$mrow" | grep -q "Alpha.*s1 s2 S3"'
ok "plugins: detail after an invalid id is dropped" '! echo "$mrow" | grep -q d-dropped && echo "$mrow" | grep -q d-a && echo "$mrow" | grep -q Cee'
rm -rf "$PL/multi"
mkplug sec 'printf "a\tAlpha\nsection\tX\nb\tBeta\n"'
mkplug crl 'printf "a\tAl\033[31mpha\001\tst\r\n  \td\033[1mx\r\n"'
srow=$(_ai_plugin_rows sec | plain); crow=$(_ai_plugin_rows crl)
ok "plugins: a later section line is ignored" '! echo "$srow" | grep -q "^plugin:sec${TAB}section" && echo "$srow" | grep -q Beta'
ok "plugins: CR and control chars stripped" 'echo "$crow" | plain | grep -q "Al\[31mpha" && ! echo "$crow" | grep -q "$(printf "\r")" && ! echo "$crow" | grep -q "$(printf "\001")"'
rm -rf "$PL/sec" "$PL/crl"
mkdir -p "$T/src/myp" "$T/src/noplug" "$T/keepplug"; printf '#!/bin/sh\necho mine\n' > "$T/src/myp/plugin"; chmod +x "$T/src/myp/plugin"
AI() { "$REPO/bin/ai" "$@"; }
# cliproxy's setup against fakes (no network, no real brew): a brew without cliproxyapi, a cliproxyapi whose start
# marks the proxy up, and a curl whose probe is up after that start and whose API calls answer 200
mkdir -p "$T/cpfake"
printf '#!/bin/sh\ntouch "%s/cpup"\n' "$T" > "$T/cpfake/cliproxyapi"
printf '#!/bin/sh\ncase " $* " in *" -w "*) printf "\\n200" ;; *) [ -e "%s/cpup" ] ;; esac\n' "$T" > "$T/cpfake/curl"
printf '#!/bin/sh\nexit 1\n' > "$T/cpfake/brew"; chmod +x "$T"/cpfake/*
AIcp() { rm -f "$T/cpup"; PATH=$T/cpfake:$PATH AI_YES=1 CLIPROXY_SETUP_ACCOUNTS=none AI "$@" </dev/null; }
ok "plugin add: local folder" 'AI plugin add "$T/src/myp/" >/dev/null 2>&1 && [ -x "$PL/myp/plugin" ] && [ ! -e "$PL/myp/.source" ]'
mkdir -p "$T/src/real"; cp "$T/src/myp/plugin" "$T/src/real/"; ln -s real "$T/src/lnkp"
ok "plugin add: symlinked folder copies contents" 'AI plugin add "$T/src/lnkp" >/dev/null 2>&1 && [ -d "$PL/lnkp" ] && [ ! -L "$PL/lnkp" ] && [ -x "$PL/lnkp/plugin" ]; rm -rf "$PL/lnkp"'
echo changed > "$T/src/myp/plugin"
ok "plugin add: refuses to overwrite" '! AI plugin add "$T/src/myp" >/dev/null 2>&1 && grep -q mine "$PL/myp/plugin"'
ok "plugin add: refuses a folder without plugin" '! AI plugin add "$T/src/noplug" >/dev/null 2>&1 && [ ! -e "$PL/noplug" ]'
ok "plugin add: bundled cliproxy" 'AIcp plugin add cliproxy >/dev/null 2>&1 && [ -x "$PL/cliproxy/plugin" ] && grep -q "^mgmt_key=" "$PL/cliproxy/config" && grep -qx proxy_config=run/config.yaml "$PL/cliproxy/config"'
mkdir -p "$T/src/ran"; printf '#!/bin/sh\ntouch "%s/ran.rows"\n' "$T" > "$T/src/ran/plugin"; chmod +x "$T/src/ran/plugin"
ok "plugin add: runs rows once at install" 'AI plugin add "$T/src/ran" >/dev/null 2>&1 && [ -e "$T/ran.rows" ]'
mkdir -p "$T/src/bad.name"; cp "$T/src/ran/plugin" "$T/src/bad.name/"
ok "plugin add: folder basename with a dot refused" '! AI plugin add "$T/src/bad.name" >/dev/null 2>&1 && ! AI plugin add . >/dev/null 2>&1 && ! AI plugin add .. >/dev/null 2>&1'
ok "plugin add: folder named like owner/repo wins over git" 'mkdir -p "$T/wd/own/repo" && cp "$T/src/ran/plugin" "$T/wd/own/repo/" && (cd "$T/wd" && AI plugin add own/repo >/dev/null 2>&1) && [ -x "$PL/repo/plugin" ] && [ ! -e "$PL/repo/.source" ]'
ok "plugin add: ssh refused" '[ "$(AI plugin add git@github.com:o/r.git 2>&1)" = "use an https:// URL" ] && [ "$(AI plugin add ssh://h/o/r 2>&1)" = "use an https:// URL" ]'
ok "plugin add: unusable source refused" '! AI plugin add "no such/thing" >/dev/null 2>&1 && ! AI plugin add a/b/c >/dev/null 2>&1 && ! AI plugin add http://x/y >/dev/null 2>&1 && ! AI plugin add nothere >/dev/null 2>&1'
ok "plugin update: needs .source and .git" 'mkdir -p "$PL/ran/.git" && ! AI plugin update ran >/dev/null 2>&1 && rmdir "$PL/ran/.git" && echo u > "$PL/ran/.source" && ! AI plugin update ran >/dev/null 2>&1; rm -f "$PL/ran/.source"'
ok "plugin add: bad names refused" '! AI plugin add "$T/src/myp" ../x >/dev/null 2>&1 && ! AI plugin add "$T/src/myp" a/b >/dev/null 2>&1 && ! AI plugin add "$T/src/myp" "a b" >/dev/null 2>&1 && [ ! -e "${PL%/*}/x" ] && [ ! -e "$PL/a" ] && [ ! -e "$PL/a b" ]'
ok "plugin: other commands refuse bad names" '! AI plugin rm ../plugins <<<y >/dev/null 2>&1 && ! AI plugin off a/b >/dev/null 2>&1 && ! AI plugin do "../x" y >/dev/null 2>&1 && [ -d "$PL" ]'
if has git; then
  G=$T/gsrc; git init -q "$G"; printf '#!/bin/sh\necho v1\n' > "$G/plugin"; chmod +x "$G/plugin"
  git -C "$G" add plugin; git -C "$G" -c user.email=t@example.com -c user.name=t commit -qm v1
  git clone -q --bare "$G" "$T/g.git"
  ok "plugin add: git url writes .source" 'AI plugin add "file://$T/g.git" gp >/dev/null 2>&1 && [ "$(cat "$PL/gp/.source")" = "file://$T/g.git" ] && grep -q v1 "$PL/gp/plugin"'
  printf '#!/bin/sh\necho v2\n' > "$G/plugin"; git -C "$G" -c user.email=t@example.com -c user.name=t commit -qam v2; git -C "$G" push -q "$T/g.git" HEAD 2>/dev/null
  ok "plugin update: pulls" 'AI plugin update gp >/dev/null 2>&1 && grep -q v2 "$PL/gp/plugin"'
  ok "plugin add: git name from url with trailing slash" 'AI plugin add "file://$T/g.git/" >/dev/null 2>&1 && [ -x "$PL/g/plugin" ]; rm -rf "$PL/g"'
  printf 'true\n' > "$G/setup"; git -C "$G" add setup; git -C "$G" -c user.email=t@example.com -c user.name=t commit -qm setup; git -C "$G" push -q "$T/g.git" HEAD 2>/dev/null
  ok "setup: update says setup changed" 'AI plugin update gp 2>&1 | grep -qx "setup changed: run setup again to apply" && ! AI plugin update gp 2>&1 | grep -q "setup changed"'
fi
ok "plugin off/on writes .off" 'AI plugin off myp >/dev/null && [ -e "$PL/myp/.off" ] && AI plugin on myp >/dev/null && [ ! -e "$PL/myp/.off" ]'
mkdir -p "$PL/usr"; printf '#!/bin/sh\n:\n' > "$PL/usr/plugin"; chmod +x "$PL/usr/plugin"; echo "https://bob:s3cret@example.com/x.git" > "$PL/usr/.source"
ok "plugin ls: userinfo hidden in source" '! AI plugin ls 2>&1 | grep -q s3cret && AI plugin ls 2>&1 | grep -q "https://example.com/x.git"'
rm -rf "$PL/usr"
ok "plugin ls: on/off" 'AI plugin ls 2>&1 | plain | grep "^off " | grep -q off && AI plugin ls 2>&1 | plain | grep "^good " | grep -q "GOOD STUFF"'
out=$(AI plugin do good x 2>&1)
ok "plugin do: captures stdout and stderr" 'case $out in *"did x"*oops*) true;; *) false;; esac'
s=$(date +%s); AI_PLUGIN_DO_TIMEOUT=1 AI plugin do slow x >/dev/null 2>&1; dorc=$?; took=$(( $(date +%s) - s ))
ok "plugin do: killed on timeout" '[ $dorc != 0 ] && [ $took -lt 4 ]'
ok "plugin do: off plugin refused" '! AI plugin do off x >/dev/null 2>&1 && [ ! -e "$T/off.ran" ]'
ok "plugin rm: n keeps it" '! AI plugin rm myp <<<n >/dev/null 2>&1; [ -d "$PL/myp" ]'
ok "plugin rm: y removes it" 'AI plugin rm myp <<<y >/dev/null 2>&1 && [ ! -e "$PL/myp" ]'
ok "plugin rm: AI_YES=1 skips the ask" 'AI_YES=1 AI plugin rm cliproxy </dev/null >/dev/null 2>&1 && [ ! -e "$PL/cliproxy" ]'
touch "$T/keepplug/plugin"; ln -s "$T/keepplug" "$PL/lnk"
ok "plugin rm: a link drops only the link" 'AI_YES=1 AI plugin rm lnk </dev/null >/dev/null 2>&1 && [ ! -L "$PL/lnk" ] && [ -f "$T/keepplug/plugin" ]'
ok "default: plugin id accepted" '_ai_default plugin:good/a >/dev/null && [ "$(cat "$AI_FO_ORDER.default")" = plugin:good/a ]'
ok "default: plugin launches with args" '[ "$(cd "$HOME/repo/a"; ai_main --z 2>&1)" = "launch a --z in $PL/good at $HOME/repo/a" ]'
ok "default: bad plugin ids refused" '! _ai_default_ok plugin:nope/a && ! _ai_default_ok plugin:good/ && ! _ai_default_ok "plugin:../good/a" && ! _ai_default_ok "plugin:good/a b"'
: > "$PL/good/.off"; : > "$FZ"   # the picker stub cancels at once
ok "default: off plugin -> picker" '[ -z "$(pick 2>&1)" ]'
rm -f "$PL/good/.off"; mv "$PL/good" "$T/good.away"
ok "default: removed plugin -> picker" '[ -z "$(pick 2>&1)" ]'
mv "$T/good.away" "$PL/good"; _ai_default - >/dev/null
ok "failover never lists plugins" '! _fo_list | grep -q plugin'

o=$(_ai_plugin_add https://tokentok@127.0.0.1:9/x.git leak 2>&1); rm -rf "$PL/leak"
ok "plugins: add never prints the URL token" '! echo "$o" | grep -q tokentok && echo "$o" | grep -q "git clone failed"'
ok "plugins: sanitizer strips userinfo anywhere" '[ "$(echo "x '"'"'https://u:tokentok@h/p'"'"' y" | _ai_plugin_nouser)" = "x '"'"'https://h/p'"'"' y" ]'

# ── plugin setup scripts: review, staging, rollback ──
mksetup() { mkdir -p "$T/src/$1"; printf '#!/bin/sh\necho hi\n' > "$T/src/$1/plugin"; chmod +x "$T/src/$1/plugin"; printf '%s\n' "$2" > "$T/src/$1/setup"; }
okset='echo "cfg $(date +%s%N)" >"$AI_PLUGIN_DIR/config"; pwd >"'"$T"'/setup.log"'
mksetup withsetup "$okset"
o=$(AI_YES=1 _ai_plugin_add "$T/src/withsetup" 2>&1)
ok "setup: approved runs in staging, then installs" '[ -f "$PL/withsetup/config" ] && case $(cat "$T/setup.log") in */.withsetup.install) true;; *) false;; esac && [ ! -e "$PL/.withsetup.install" ] && echo "$o" | grep -q "SETUP SCRIPT · review before running"'
mksetup withsetup2 "$okset"
ok "setup: refused leaves nothing" '! echo n | _ai_plugin_add "$T/src/withsetup2" >/dev/null 2>&1 && [ ! -e "$PL/withsetup2" ] && [ ! -e "$PL/.withsetup2.install" ] && [ "$(echo n | _ai_plugin_add "$T/src/withsetup2" 2>&1 | tail -1)" = "cancelled: plugin not installed" ]'
mksetup failset 'exit 3'
o=$(AI_YES=1 _ai_plugin_add "$T/src/failset" 2>&1); rc=$?
ok "setup: failing setup leaves nothing" '[ $rc != 0 ] && echo "$o" | grep -qx "setup failed (exit 3): plugin not installed" && [ ! -e "$PL/failset" ] && [ ! -e "$PL/.failset.install" ]'
mksetup lnkset x; rm "$T/src/lnkset/setup"; printf 'touch "%s/evil.ran"\n' "$T" > "$T/evil"; ln -s "$T/evil" "$T/src/lnkset/setup"
o=$(AI_YES=1 _ai_plugin_add "$T/src/lnkset" 2>&1)
ok "setup: a linked setup is never run" 'echo "$o" | grep -qx "setup is a link: not run" && [ ! -e "$T/evil.ran" ] && [ ! -e "$PL/lnkset" ] && [ ! -e "$PL/.lnkset.install" ]'
printf 'printf "\\033[2J"\n\033[2Jboom\n' > "$T/escset"
o=$(_ai_plugin_review "$T/escset")
ok "setup: review escapes control bytes" 'case $o in *"^[[2Jboom"*) true;; *) false;; esac && ! printf "%s" "$o" | grep -q "$(printf "\033")" && echo "$o" | grep -q "^ *2 "'
mksetup withsetup3 "$okset"; mkdir -p "$PL/.withsetup3.install"; touch "$PL/.withsetup3.install/junk" "$PL/.withsetup3.install/plugin"
ok "setup: stale staging is replaced" 'AI_YES=1 _ai_plugin_add "$T/src/withsetup3" >/dev/null 2>&1 && [ -f "$PL/withsetup3/config" ] && [ ! -e "$PL/withsetup3/junk" ] && [ ! -e "$PL/.withsetup3.install" ]'
mkdir -p "$PL/.dotted"; cp "$T/src/withsetup/plugin" "$PL/.dotted/"
ok "setup: dot folders are never plugins" '! _ai_plugin_names | grep -q "^\."; rm -rf "$PL/.dotted"'
cp "$PL/withsetup/config" "$T/config.prev"; sleep 1
ok "setup: plugin setup reruns and keeps config.bak" 'AI_YES=1 AI plugin setup withsetup >/dev/null 2>&1 && cmp -s "$PL/withsetup/config.bak" "$T/config.prev" && ! cmp -s "$PL/withsetup/config" "$T/config.prev" && [ "$(cat "$T/setup.log")" = "$PL/withsetup" ]'
printf 'exit 4\n' > "$PL/withsetup/setup"
ok "setup: failed rerun keeps the plugin" '[ "$(AI_YES=1 AI plugin setup withsetup 2>&1 | tail -1)" = "setup failed (exit 4)" ] && [ -x "$PL/withsetup/plugin" ]'
ok "setup: refused rerun runs nothing" 'printf "touch %s/rerun.ran\n" "$T" > "$PL/withsetup/setup" && ! AI plugin setup withsetup <<<n >/dev/null 2>&1 && [ ! -e "$T/rerun.ran" ]'
printf 'touch %s/bak.ran\n' "$T" > "$PL/withsetup/setup"; rm -f "$PL/withsetup/config.bak"; : > "$PL/withsetup/config.bak"; chmod 400 "$PL/withsetup/config.bak"
ok "setup: a failed config backup stops before setup" '! AI_YES=1 AI plugin setup withsetup >/dev/null 2>&1 && [ ! -e "$T/bak.ran" ]; r=$?; chmod 600 "$PL/withsetup/config.bak"; rm -f "$PL/withsetup/config.bak"; [ $r = 0 ]'
ok "setup: plugin setup needs a setup" '! AI plugin setup myp >/dev/null 2>&1 && ! AI plugin setup nope >/dev/null 2>&1'
mksetup sleepy 'touch "'"$T"'/setup.started"; sleep 30'; mkdir -p "$T/tmpx"
( set -m; TMPDIR=$T/tmpx AI_YES=1 "$REPO/bin/ai" plugin add "$T/src/sleepy" >"$T/int.out" 2>&1 & p=$!
  i=0; while [ ! -e "$T/setup.started" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  kill -INT -- -$p; wait $p ) 2>/dev/null
ok "setup: interrupt removes staging and the temp copy" '[ -e "$T/setup.started" ] && [ ! -e "$PL/sleepy" ] && [ ! -e "$PL/.sleepy.install" ] && grep -qx "setup failed (exit 130): plugin not installed" "$T/int.out" && [ -z "$(ls -A "$T/tmpx")" ]'

# plugin new, about, available list
ok "plugin new: creates a working plugin" 'AI plugin new demo >/dev/null 2>&1 && [ -x "$PL/demo/plugin" ] && [ -x "$PL/demo/setup" ] && "$PL/demo/plugin" rows | head -1 | grep -q "^section${TAB}" && _ai_plugin_rows demo | cut -f1,2 | grep -q "^plugin:demo${TAB}hello"'
cp -R "$PL/demo" "$T/demo.bak"
ok "plugin new: refuses existing and bad names" '! AI plugin new demo >/dev/null 2>&1 && ! AI plugin new "a b" >/dev/null 2>&1 && diff -r "$PL/demo" "$T/demo.bak" >/dev/null'
lsout=$(AI plugin ls 2>&1 | plain)
ok "plugin ls: lists available bundled with about" 'echo "$lsout" | grep -A3 "^available" | grep -q "cliproxy.*Claude Code through CLIProxyAPI: all accounts as one (has setup)"'
ok "plugin ls: installed bundled leaves available" 'AIcp plugin add cliproxy >/dev/null 2>&1 && AI plugin ls 2>&1 | plain | grep -q "no other bundled plugins"'
AI_PLUGIN_DIR=$PL/demo "$PL/demo/setup" >/dev/null 2>&1; cp "$PL/demo/config" "$T/demo.cfg"
ok "template setup is safe to run twice" 'AI_PLUGIN_DIR=$PL/demo "$PL/demo/setup" >/dev/null 2>&1 && cmp -s "$PL/demo/config" "$T/demo.cfg"'

# the bundled cliproxy plugin has its own suite (fake proxy, fake HOME)
ok "cliproxy plugin suite" 'bash "$REPO/tests/cliproxy.sh" >/dev/null 2>&1'

echo; if [ $fails -gt 0 ]; then echo "$fails failed"; exit 1; else echo "all passed"; fi
