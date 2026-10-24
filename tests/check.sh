#!/usr/bin/env bash
# Self-check: runs every feature against a throwaway fake HOME. Never touches your real configs.
#   bash tests/check.sh        exit 0 = all passed   (also run it with /bin/bash on macOS: bash 3.2)
REPO=$(cd "$(dirname "$0")/.." && pwd)
T=$(cd "$(mktemp -d)" && pwd -P); trap 'rm -rf "$T"' EXIT   # resolved path: agents store getcwd()
export HOME=$T/home CLA_ROOT=$T/home/acc CODEX_HOME=$T/home/.codex AI_PROVIDERS=$T/cfg/providers AI_ACCOUNTS=$T/home/.ai-accounts
mkdir -p "$HOME" "$CLA_ROOT/one/usage" "$CLA_ROOT/two/usage" "$CODEX_HOME/sessions/2026/01/01" \
  "$HOME/.gemini/antigravity-cli" "$HOME/repo/a" "$HOME/repo/b"
. "$REPO/lib/core.sh"; . "$REPO/lib/trust.sh"; . "$REPO/lib/failover.sh"; for f in "$REPO"/plugins/*.sh; do . "$f"; done
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
ok "main login stays the default" '[ "$(_ai_run codex codex)" = "$CODEX_HOME" ]'
_ai_add other "aider" >/dev/null <<<""
ok "other CLIs never get a login folder" '[ ! -e "$AI_ACCOUNTS/aider" ]'
ok "names must be path-safe"    '! _ai_add .. codex x >/dev/null && ! _ai_add a/b codex x >/dev/null'
_ai_put work; _ai_add work codex "Codex work" >/dev/null
ok "re-adding a name keeps its login" '[ "$(_ai_account_dir work)" = "$AI_ACCOUNTS/codex/work" ]'

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

# ── failover ──
AI_FO_WAIT=0; FO=$T/fo; mkdir -p "$FO/bin" "$FO/proj/-slug" "$FO/state/limited/claude"; AI_FO_STATE=$FO/state; AI_FO_ORDER=$T/cfg/failover; rm -f "$AI_FO_ORDER" "$AI_FO_ORDER.on"
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
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi --model m ) 2>/dev/null
ok "run: next account resumes the session" 'grep -q "^$CLA_ROOT/two|.*--model m --resume S1" "$FO_LOG"'
ok "run: transcript copied to its config dir" '[ -f "$CLA_ROOT/two/projects/-slug/S1.jsonl" ]'
ok "run: limit is remembered"     '_fo_limited claude/one >/dev/null'
# claude/two off: the next stop is the codex provider, which gets a handoff with the transcript
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; rm -f "$FO_LOG.codex"
_fo_list | _fo_set claude/two off | _fo_save
( PATH=$FO/bin:$PATH; _fo_run claude/one -p hi ) 2>/dev/null
ok "run: codex gets a handoff"    'grep -q "hello world" "$FO_LOG.codex" && grep -q "half done" "$FO_LOG.codex"'
# q at the switch card: stops, next member never launched
_ai_failover clear >/dev/null 2>&1; : > "$FO_LOG"; _fo_list | _fo_set claude/two on | _fo_save
( PATH=$FO/bin:$PATH; AI_FO_WAIT=2 _fo_run claude/one -p hi ) 2>/dev/null <<<"q"; stoprc=$?
ok "run: q at the switch card stops" '[ $stoprc != 0 ] && [ "$(wc -l < "$FO_LOG" | tr -d " ")" = 1 ] && ! grep -q "^$CLA_ROOT/two|" "$FO_LOG"'
mrows=$(_fo_rows)
ok "menu order: u 2 is up"        '[ "$(_ai_menu t "" order "$mrows" <<<"u 2" 2>/dev/null | head -1)" = up ]'
ok "menu order: x 1 is toggle"    '[ "$(_ai_menu t "" order "$mrows" <<<"x 1" 2>/dev/null | head -1)" = toggle ]'
ok "menu keys: x 1 is delete"     '[ "$(_ai_menu t "" keys "$mrows" <<<"x 1" 2>/dev/null | head -1)" = delete ]'
ok "menu keys: u 2 fails"         '! _ai_menu t "" keys "$mrows" <<<"u 2" >/dev/null 2>&1'
_ai_failover off >/dev/null 2>&1

# ── failover: screens ──
vis() { plain | wc -m | tr -d ' '; }
card=$(_fo_card "T" "short" $'\033[1mbold\033[0m ünï ✻ wide line here' 2>&1 >/dev/null | plain | sed '/^$/d')
ok "card: box lines share one width" '[ "$(echo "$card" | while IFS= read -r l; do printf "%s" "$l" | wc -m; done | sort -u | wc -l | tr -d " ")" = 1 ]'
ok "card: has top, body and bottom" '[ "$(echo "$card" | wc -l | tr -d " ")" = 4 ] && echo "$card" | line 1 | grep -q "^╭" && echo "$card" | line 4 | grep -q "^╰"'
ok "label: claude account"        '[ "$(_fo_label claude/x)" = "Claude account x" ]'
ok "label: provider with label"   '[ "$(_fo_label cl2)" = "Claude 2 (cl2)" ]'
ok "label: provider without label" '[ "$(_fo_label other2)" = other2 ]'
ok "wait: enter goes"             '[ "$(AI_FO_WAIT=2 _fo_wait 2>/dev/null <<<"")" = go ]'
ok "wait: o is order"             '[ "$(AI_FO_WAIT=2 _fo_wait 2>/dev/null <<<"o")" = order ]'
ok "wait: q is stop"              '[ "$(AI_FO_WAIT=2 _fo_wait 2>/dev/null <<<"q")" = stop ]'
ok "wait: 0 seconds goes without input" '[ "$(AI_FO_WAIT=0 _fo_wait 2>/dev/null </dev/null)" = go ]'
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

echo; if [ $fails -gt 0 ]; then echo "$fails failed"; exit 1; else echo "all passed"; fi
