# shellcheck shell=bash
# ai-launcher core. Plain bash 3.2+ (the bash built into macOS), so nothing has to be installed.
# Optional: fzf (nicer picker), jq (usage bars + shared trust), tmux (Claude team mode).

AI_PROVIDERS=${AI_PROVIDERS:-${XDG_CONFIG_HOME:-$HOME/.config}/ai-launcher/providers}
AI_ACCOUNTS=${AI_ACCOUNTS:-$HOME/.ai-accounts}
AI_DEFAULT_ORDER="claude codex gemini agy kimi opencode qwen"
TAB=$'\t'
NL=$'\n'

has() { command -v "$1" >/dev/null 2>&1; }
# Git Bash on Windows (MSYS/MinGW) or Cygwin
is_windows() { case $(uname -s) in MINGW*|MSYS*|CYGWIN*) return 0 ;; esac; return 1; }
# path for native (non-bash) programs: C:/Users/... on Windows, unchanged elsewhere
native_path() { if has cygpath; then cygpath -m "$1"; else printf '%s\n' "$1"; fi; }
is_fn() { [ "$(type -t "$1")" = function ]; }
_ai_default_label() {
  case $1 in
    claude) echo "Claude Code" ;; codex) echo "Codex CLI" ;; gemini) echo "Gemini CLI" ;;
    agy) echo "Antigravity" ;; kimi) echo "Kimi Code" ;; opencode) echo "OpenCode" ;;
    qwen) echo "Qwen Code" ;; *) echo "$1" ;;
  esac
}
# env var that moves an agent's config (and login) to another folder. Claude has its own accounts (cla).
_ai_account_env() { case $1 in codex) echo CODEX_HOME ;; gemini) echo GEMINI_CLI_HOME ;; esac; }
# the CLI a provider runs: first word of its command after any VAR=value, without its folder
_ai_agent() {
  _ai_get "$1" 2 | awk '{for (i = 1; i <= NF; i++) if ($i !~ /^[A-Za-z_][A-Za-z0-9_]*=/) {n = split($i, p, "/"); print p[n]; exit}}'
}
# a provider's own login folder, if it has one. `ai add` makes it for the 2nd+ provider of the same CLI.
_ai_account_dir() {
  local a; a=$(_ai_agent "$1")
  [ -n "$(_ai_account_env "$a")" ] && [ -d "$AI_ACCOUNTS/$a/$1" ] && echo "$AI_ACCOUNTS/$a/$1"
}

# ── update notice: shares the TUI's cache `<state>/ai-launcher/update-check` (`<secs>\t<version>`, 24h) ──
# "major minor patch release" for x.y.z[-pre] (release=1, prerelease=0); fails on anything else
_ai_semver() {
  local v=${1#v} core pre=1
  case $v in *-*) core=${v%%-*}; pre=0 ;; *) core=$v ;; esac
  case $core in *[!0-9.]*|.*|*.|*..*) return 1 ;; esac
  local IFS=.; set -- $core
  [ $# = 3 ] && echo "$1 $2 $3 $pre"
}
# true when $1 is a strictly newer release than $2
_ai_is_newer() {
  local a b; a=$(_ai_semver "$1") && b=$(_ai_semver "$2") || return 1
  set -- $a $b
  local i x y
  for i in 1 2 3 4; do
    eval "x=\${$i} y=\${$((i + 4))}"
    [ "$x" -gt "$y" ] && return 0; [ "$x" -lt "$y" ] && return 1
  done
  return 1
}
# prints the notice line when a newer release is cached; refreshes a stale cache in the background
_ai_update_notice() {
  [ -n "${AI_LAUNCHER_NO_UPDATE_CHECK:-}" ] && return 0
  local cur t v now cache
  cur=$(sed -n 's/^version *= *"\(.*\)".*/\1/p' "${AI_HOME:-/nonexistent}/tui/Cargo.toml" 2>/dev/null | head -1)
  [ -n "$cur" ] || return 0
  cache=$(dirname "${AI_FO_STATE:-${XDG_STATE_HOME:-$HOME/.local/state}/ai-launcher/failover}")/update-check
  now=$(date +%s)
  t=; v=; [ -r "$cache" ] && IFS=$'\t' read -r t v < "$cache"
  case $t in ''|*[!0-9]*) t=0 ;; esac
  if [ $((now - t)) -ge 86400 ] && has npm; then
    ( { v=$(npm view ai-launcher version </dev/null 2>/dev/null) && _ai_semver "$v" >/dev/null &&
        mkdir -p "$(dirname "$cache")" && printf '%s\t%s\n' "$now" "$v" > "$cache"; } >/dev/null 2>&1 & )
  elif [ $((now - t)) -lt 86400 ] && _ai_is_newer "$v" "$cur"; then
    echo "update $v available · npm i -g ai-launcher"
  fi
  return 0
}

ai_main() {
  case ${1:-} in
    ls|add|edit|rm|failover|default|trust) local sub=$1; shift; "_ai_$sub" "$@"; return ;;
    cla) shift; cla_main "$@"; return ;;
    cla-team) shift; cla_team_main "$@"; return ;;
    help|-h|--help) _ai_help; return ;;
  esac
  local out key row name id d
  if [ "${1:-}" = pick ]; then shift
  elif d=$(head -1 "$AI_FO_ORDER.default" 2>/dev/null) && _ai_default_ok "$d"; then
    case $d in claude/*) _ai_run claude "${d#claude/}" "$@" ;; *) _ai_run "$d" "$d" "$@" ;; esac
    return
  fi
  while :; do
    AI_NOTICE=$(_ai_update_notice)
    _ai_seed
    [ -s "$AI_PROVIDERS" ] || { echo "no providers. add one: ai add <name> [command]"; return 1; }
    out=$(_ai_choose " ✻ AI agents · failover $(_fo_state) " \
      'enter launch · ctrl-t claude team · ctrl-a add · ctrl-e edit · ctrl-x delete · ctrl-f failover on/off · ctrl-o failover order' \
      keys "$(_ai_rows)") || return 0
    key=${out%%"$NL"*}; row=${out#*"$NL"}
    name=${row%%"$TAB"*}; id=${row#*"$TAB"}; id=${id%%"$TAB"*}
    case $key in
      add) _ai_add ;;
      edit) _ai_edit "$name" ;;
      delete) _ai_rm "$name" ;;
      failover) _fo_toggle 2>/dev/null || { _fo_toggle; sleep 2; } ;;   # the title shows the new state
      order) _fo_ui ;;
      *) [ "$key" = team ] && export AI_TEAM=1
         _ai_run "$name" "$id" "$@"; return ;;
    esac
  done
}

_ai_help() {
  cat <<'EOF'
ai                               pick an agent (with plan usage) and start it in this folder,
                                 or start the default one straight away if set
ai [agent args]                  same, extra args go to the agent
ai pick [agent args]             always show the picker, even with a default
ai default [name|-]              show / set / clear the default: claude/<account> or a provider
ai ls                            list agents
ai add  [name] [command] [label] add an agent (asks for anything left out). A 2nd codex or
                                 gemini gets its own login: ai add work codex
ai edit <name> [command] [label] change its command / label
ai rm   <name>                   delete an agent
ai failover [on|off|ls]          when a Claude account hits its limit, resume the session on the
                                 next account (Codex last). No args: set priority, turn accounts on/off
ai failover order <member>...    set priority from the command line
ai failover auto [on|off]        at a limit, move by itself after a countdown (on) or ask y/N (off,
                                 default). A move to Codex always asks
ai trust [on|off|status]         share folder trust between agents (off by default)
cla [name] [team] [args]         Claude accounts: pick or start one   (cla add|ls|rm|mv)
cla-team [name] [args]           Claude team mode (needs tmux)

Picker keys: enter launch · ctrl-t team · ctrl-a add · ctrl-e edit · ctrl-x delete
             ctrl-f failover on/off · ctrl-o failover order
Without fzf you get a numbered menu: 2 = launch, t 2 = team, a = add, e 2 = edit, x 2 = delete,
f = failover on/off, o = failover order.
EOF
}

_ai_trust() {
  local flag=${AI_PROVIDERS%/*}/trust.on
  case ${1:-status} in
    on) mkdir -p "${flag%/*}" && : > "$flag"
        echo "shared trust on: a folder trusted in one agent is now trusted in all of them."
        echo "Trusting a folder lets each agent load that folder's hooks, settings and MCP servers," 
        echo "so only turn this on if you trust every folder you open. Turn off: ai trust off" ;;
    off) rm -f "$flag"; echo "shared trust off" ;;
    status) if [ -e "$flag" ]; then echo "shared trust: on"; else echo "shared trust: off (ai trust on)"; fi ;;
    *) echo "usage: ai trust [on|off|status]"; return 1 ;;
  esac
}

# ── providers file: name TAB command TAB label ─────────────
_ai_seed() {
  [ -e "$AI_PROVIDERS" ] && return 0
  mkdir -p "$(dirname "$AI_PROVIDERS")"
  local n
  for n in $AI_DEFAULT_ORDER; do
    has "$n" && printf '%s\t%s\t%s\n' "$n" "$n" "$(_ai_default_label "$n")"
  done > "$AI_PROVIDERS"
}
_ai_names() { _ai_seed; cut -f1 "$AI_PROVIDERS"; }
# _ai_get <name> <field: 2 = command, 3 = label>
_ai_get() { _ai_seed; awk -F'\t' -v k="$1" -v f="$2" '$1 == k {print $f; exit}' "$AI_PROVIDERS"; }
# _ai_put <name> [command label]; no command = delete. Keeps the row's position.
_ai_put() {
  local out
  out=$(K=$1 C=${2:-} L=${3:-} awk -F'\t' -v OFS='\t' '
    $1 == ENVIRON["K"] { if (ENVIRON["C"] != "") print ENVIRON["K"], ENVIRON["C"], ENVIRON["L"]; done = 1; next }
    { print } END { if (!done && ENVIRON["C"] != "") print ENVIRON["K"], ENVIRON["C"], ENVIRON["L"] }' "$AI_PROVIDERS")
  printf '%s\n' "$out" | grep . > "$AI_PROVIDERS"
  return 0
}
# launch command: saved one, else the default given (cla uses it even if claude was deleted)
_ai_cmd() { local c; c=$(_ai_get "$1" 2); printf '%s\n' "${c:-$2}"; }

# ask with the current value shown; Enter keeps it (bash 3.2 has no prefilled input)
_ai_ask() {
  local ans
  if [ -n "$2" ]; then read -r -p "$1 [$2]: " ans || return 1
  else read -r -p "$1: " ans || return 1; fi
  printf '%s\n' "${ans:-$2}"
}

_ai_ls() {
  _ai_seed
  local n c l u
  while IFS="$TAB" read -r n c l; do
    u=; is_fn "_ai_${n}_rows" && u="  · usage"
    printf '\033[1;36m%s\033[0m %s \033[2m%s%s\033[0m\n' "$(pad "$n" 9)" "$(pad "$c" 34)" "$l" "$u"
  done < "$AI_PROVIDERS"
}
_ai_add() {
  _ai_seed
  local n=${1:-} c=${2:-} l=${3:-}
  [ -n "$n" ] || n=$(_ai_ask "name") || return 1
  case $n in ''|.|..|*[!A-Za-z0-9._-]*) echo "name must be one word (letters, digits, . _ -)"; return 1 ;; esac
  [ -n "$(_ai_get "$n" 1)" ] && { echo "'$n' exists. change it: ai edit $n"; return 1; }
  [ -n "$c" ] || c=$(_ai_ask "command" "$n") || return 1
  [ -n "$l" ] || l=$(_ai_ask "label" "$(_ai_default_label "$n")") || return 1
  [ -n "$c" ] || { echo "command is required"; return 1; }
  _ai_put "$n" "$c" "$l" && echo "added $n -> $c"
  _ai_add_login "$n"
}
# Another provider already runs this CLI: give this one its own login folder, so it is a
# separate account. `ai rm` deletes that folder with the provider.
_ai_add_login() {
  local a o d; a=$(_ai_agent "$1")
  [ -n "$(_ai_account_env "$a")" ] || return 0
  for o in $(_ai_names); do
    [ "$o" != "$1" ] && [ "$(_ai_agent "$o")" = "$a" ] && [ -z "$(_ai_account_dir "$o")" ] || continue
    d=$AI_ACCOUNTS/$a/$1
    mkdir -p "$d" && echo "separate $a login in $d. start $1 to log in."
    return 0
  done
}
_ai_edit() {
  local n=${1:-} c=${2:-} l=${3:-}
  [ -n "$n" ] && [ -n "$(_ai_get "$n" 1)" ] || { echo "no provider '$n'. see: ai ls"; return 1; }
  [ -n "$c" ] || c=$(_ai_ask "command" "$(_ai_get "$n" 2)") || return 1
  [ -n "$l" ] || l=$(_ai_ask "label" "$(_ai_get "$n" 3)") || return 1
  _ai_put "$n" "$c" "$l" && echo "saved $n -> $c"
}
_ai_rm() {
  local n=${1:-} ans d=
  [ -n "$n" ] && [ -n "$(_ai_get "$n" 1)" ] || { echo "no provider '$n'. see: ai ls"; return 1; }
  # its own codex/gemini login, only ever $AI_ACCOUNTS/<agent>/<name> with a path-safe name (never ~/.codex)
  # the provider named after its CLI (codex, gemini) is the main login: never delete a folder by that name
  case $n in .*|*[!A-Za-z0-9._-]*) ;; *) [ "$n" = "$(_ai_agent "$n")" ] || d=$(_ai_account_dir "$n") ;; esac
  read -r -p "delete provider '$n'${d:+ and its login folder $d for good (login, history, settings)}? [y/N] " ans
  case $ans in y|Y|yes) ;; *) return 1 ;; esac
  _ai_put "$n" || return 1
  # rm -rf on a symlink path removes only the link
  if [ -n "$d" ]; then rm -rf "${d:?}" && echo "deleted $n and $d"; else echo "deleted $n (bring back: ai add $n)"; fi
}

# the entry plain `ai` starts without the picker, in <failover>.default (shared with the TUI)
_ai_default() {
  local f=$AI_FO_ORDER.default
  case ${1:-} in
    '') if [ -s "$f" ]; then head -1 "$f"; else echo "no default. set one: ai default claude/<account> | <provider>"; fi ;;
    -) rm -f "$f"; echo "default cleared" ;;
    *) _ai_default_ok "$1" || { echo "no account or provider '$1'. names: claude/<account> (cla ls) or ai ls"; return 1; }
       mkdir -p "$(dirname "$f")" && echo "$1" > "$f" && echo "default: $1" ;;
  esac
}
# claude/<account> with its folder, or a provider other than claude itself (that one is the cla picker)
_ai_default_ok() {
  case $1 in
    claude/*) case ${1#claude/} in ''|.|..|*/*) return 1 ;; esac; [ -d "$CLA_ROOT/${1#claude/}" ] ;;
    ''|claude) return 1 ;;
    *) [ -n "$(_ai_get "$1" 1)" ] ;;
  esac
}

# ── rows ───────────────────────────────────────────────────
# every provider's rows, fetched in parallel, kept in providers-file order
_ai_rows() {
  local i=0 n r
  {
    for n in $(_ai_names); do
      i=$((i + 1))
      r=_ai_${n}_rows  # else the plugin of the CLI it runs, for codex / gemini logins
      is_fn "$r" || { [ -n "$(_ai_account_env "$(_ai_agent "$n")")" ] && r=_ai_$(_ai_agent "$n")_rows; }
      if is_fn "$r" && has jq; then
        "$r" "$n" | sed "s/^/$i$TAB/" &
      else
        _ai_row "$n" "$n" "$n" "$(_ai_get "$n" 3)" "$(dim 'no usage data')" | sed "s/^/$i$TAB/"
      fi
    done
    wait
  } | sort -s -t "$TAB" -k1,1n | cut -f2-
}
# one row: <provider> TAB <id> TAB <shown text>
_ai_row() { printf '%s\t%s\t\033[1;36m%s\033[0m %s  %s\n' "$1" "$2" "$(pad "$3" 9)" "$(pad "$4" 26)" "$5"; }
# "5h ███░░░░░░░  33% ↻ Tue 12:00"; green < 60 <= yellow < 90 <= red
_ai_bar() {
  local label=${1:-} pct=${2:-} at=${3:-} c n
  [ -z "$label" ] && { printf '%33s' ''; return; }
  [ -z "$pct" ] && { printf '%s \033[2m%s\033[0m' "$(pad "$label" 3)" "$(pad '──────────  n/a' 29)"; return; }
  if [ "$pct" -ge 90 ]; then c=31; elif [ "$pct" -ge 60 ]; then c=33; else c=32; fi
  n=$(( (pct + 5) / 10 )); [ $n -gt 10 ] && n=10
  printf '%s \033[%sm%s\033[2m%s\033[0m \033[1;%sm%3d%%\033[0m \033[2m%s\033[0m' \
    "$(pad "$label" 3)" $c "$(rep █ $n)" "$(rep ░ $((10 - n)))" $c "$pct" "$(pad "${at:+↻ $at}" 11)"
}
dim() { printf '\033[2m%s\033[0m' "$1"; }
rep() { local s='' i; for ((i = 0; i < $2; i++)); do s=$s$1; done; printf '%s' "$s"; }
# pad to a visible width: ignores color codes, counts characters (not bytes) in a UTF-8 locale
pad() {
  local s=$1 v
  v=$(printf '%s' "$s" | sed $'s/\033\\[[0-9;]*m//g')
  printf '%s%*s' "$s" $(( $2 > ${#v} ? $2 - ${#v} : 0 )) ''
}

# ── picker: fzf when installed, else a numbered menu ──────
# _ai_choose <title> <hint> <keys|order|""> <rows>. Prints "<action>\n<row>". Fails on cancel.
# keys:  action = "" (launch) | team | add | edit | delete | failover | order
# order: action = "" (back) | up | down | toggle | clear | failover | auto   (the failover priority screen)
# Rows come as an argument, not stdin, so the menu can read the answer from the terminal
# on stdin and draw on stderr: no /dev/tty needed (unreliable in Git Bash on Windows).
_ai_choose() {
  if has fzf; then _ai_fzf "$@"; else _ai_menu "$@"; fi
}
_ai_fzf() {
  local out key extra=()
  case ${3:-} in
    keys) extra=(--expect=ctrl-t,ctrl-a,ctrl-e,ctrl-x,ctrl-f,ctrl-o) ;;
    order) extra=(--expect=ctrl-u,ctrl-d,ctrl-x,ctrl-r,ctrl-f,ctrl-a) ;;
  esac
  out=$(fzf --ansi --delimiter="$TAB" --with-nth=3 --height=~16 --reverse --border=rounded \
      --border-label="$1" --prompt='  ' --pointer='▶' --info=hidden --no-scrollbar \
      --header=$'\033[2m'"$2"${AI_NOTICE:+$'\n\033[38;5;173m'"$AI_NOTICE"}$'\033[0m' \
      --color='border:#d97757,label:#d97757:bold,pointer:#d97757,prompt:#d97757,hl:#d97757,hl+:#d97757' \
      ${extra[@]+"${extra[@]}"} <<<"$4") || return 1
  [ -n "${3:-}" ] || { printf '\n%s\n' "$out"; return; }
  key=${out%%"$NL"*}
  case $3:$key in
    keys:ctrl-t) key=team ;; keys:ctrl-a) key=add ;; keys:ctrl-e) key=edit ;; keys:ctrl-x) key=delete ;;
    keys:ctrl-o) key=order ;; *:ctrl-f) key=failover ;;
    order:ctrl-u) key=up ;; order:ctrl-d) key=down ;; order:ctrl-x) key=toggle ;; order:ctrl-r) key=clear ;; order:ctrl-a) key=auto ;;
  esac
  printf '%s\n%s\n' "$key" "${out#*"$NL"}"
}
_ai_menu() {
  local rows=() line i=0 ans act n mode=${3:-}
  while IFS= read -r line; do [ -n "$line" ] && rows+=("$line"); done <<<"$4"
  {
    printf '\n  \033[1;38;5;173m%s\033[0m\n' "$1"
    [ -n "${AI_NOTICE:-}" ] && printf '  \033[38;5;173m%s\033[0m\n' "$AI_NOTICE"
    printf '\n'
    for line in ${rows[@]+"${rows[@]}"}; do
      i=$((i + 1)); printf '  \033[2m%2d\033[0m  %s\n' $i "${line#*"$TAB"*"$TAB"}"
    done
    case ${3:-} in
      keys) dim "  number = launch · t N = claude team · a = add · e N = edit · x N = delete · f = failover on/off · o = failover order · q = quit" ;;
      order) dim "  u N = up · d N = down · x N = on/off · r N = clear limit · f = failover on/off · a = auto-switch on/off · q = back" ;;
      *) dim "  number = choose · q = quit" ;;
    esac
    printf '\n'
  } >&2
  read -r -p "  › " ans || return 1
  set -- $ans
  case ${1:-q} in
    q) return 1 ;;
    a) if [ "$mode" = order ]; then printf 'auto\n\n'; else printf 'add\n\n'; fi; return ;;
    f) printf 'failover\n\n'; return ;;
    o) printf 'order\n\n'; return ;;
    t) act=team; n=${2:-} ;;
    e) act=edit; n=${2:-} ;;
    x) act=delete; [ "$mode" = order ] && act=toggle; n=${2:-} ;;
    u|d|r) [ "$mode" = order ] || { echo "unknown choice: $ans" >&2; return 1; }
       case $1 in u) act=up ;; d) act=down ;; r) act=clear ;; esac; n=${2:-} ;;
    *) act=; n=$1 ;;
  esac
  case $n in ''|*[!0-9]*) echo "not a number: $ans" >&2; return 1 ;; esac
  [ "$n" -ge 1 ] && [ "$n" -le ${#rows[@]} ] || { echo "no row $n" >&2; return 1; }
  printf '%s\n%s\n' "$act" "${rows[$((n - 1))]}"
}

# true when folder $1 is $2 or inside it, compared as resolved paths
_fo_under() {
  local d r
  d=$(cd "$1" 2>/dev/null && pwd -P) && r=$(cd "$2" 2>/dev/null && pwd -P) || return 1
  case $d/ in "$r"/*) return 0 ;; esac; return 1
}

# ── launch ─────────────────────────────────────────────────
_ai_run() {
  local n=$1 id=$2 d; shift 2
  d=$(_ai_account_dir "$n") && export "$(_ai_account_env "$(_ai_agent "$n")")=$(native_path "$d")"
  # a plain launch outside any account means the main login, not one inherited from a cla / ai-account session
  if [ -z "$d" ]; then
    case $(_ai_agent "$n") in
      claude) [ -n "${CLAUDE_CONFIG_DIR:-}" ] && _fo_under "$CLAUDE_CONFIG_DIR" "$CLA_ROOT" && unset CLAUDE_CONFIG_DIR ;;
      codex) [ -n "${CODEX_HOME:-}" ] && _fo_under "$CODEX_HOME" "$AI_ACCOUNTS" && unset CODEX_HOME ;;
    esac
  fi
  [ "$n" = claude ] || _ai_trust_share "$PWD"  # claude rows go through cla, which shares too
  if is_fn "_ai_${n}_run"; then "_ai_${n}_run" "$id" "$@"
  elif [ "$(_ai_agent "$n")" = claude ] && [ -z "${AI_TEAM:-}" ] && _fo_on; then _fo_run "$n" "$@"
  else _ai_exec "$(_ai_cmd "$n" "$n")" "$@"; fi
}
# Run a saved command string plus extra args. The string is the user's own config (like a
# shell alias), so a shell parses it: quotes and env prefixes work.
# On macOS caffeinate keeps the machine awake while the agent runs.
_ai_exec() {
  local c=$1; shift
  if has caffeinate; then caffeinate -i bash -c "$c"' "$@"' ai "$@"
  else bash -c "$c"' "$@"' ai "$@"; fi
}
