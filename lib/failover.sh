# shellcheck shell=bash
# Failover: when a Claude account hits its usage limit mid-session, the same session resumes on
# the next account in priority order. A Codex account gets a written handoff instead, and is
# the last stop. Off until turned on. Needs jq.
#   ai failover                    priority screen: move, turn accounts on/off, clear a limit
#   ai failover on|off|ls
#   ai failover auto [on|off]      at a limit, move by itself after a countdown, or ask y/N (default)
#   ai failover order <member>...  put these first, in this order (names as `ai failover ls` shows)
#   ai failover clear [member]     forget a limit hit (every one without a name)
# Members: claude/<account> for each cla account, plus every provider that runs claude or codex.
AI_FO_ORDER=${AI_FO_ORDER:-${AI_PROVIDERS%/*}/failover}
AI_FO_STATE=${AI_FO_STATE:-${XDG_STATE_HOME:-$HOME/.local/state}/ai-launcher/failover}
AI_FO_COOLDOWN=${AI_FO_COOLDOWN:-18000}
AI_FO_MOD=${AI_FO_MOD:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/mods/failover}
AI_FO_PROMPT=${AI_FO_PROMPT:-"The previous account hit its usage limit and this session moved to a new account. Continue the work exactly where you left off."}

_fo_on() { [ -e "$AI_FO_ORDER.on" ] && has jq && ! is_windows; }
_fo_state() { if _fo_on; then echo on; else echo off; fi; }
# <order>.auto exists: switch by itself after AI_FO_WAIT seconds (shared with the TUI), Codex too.
_fo_auto() { [ -e "$AI_FO_ORDER.auto" ]; }
_fo_auto_word() { if _fo_auto; then echo on; else echo off; fi; }
_fo_auto_text() { if _fo_auto; then echo "auto-switch on: moves by itself after ${AI_FO_WAIT:-5}s, Codex too"; else echo "auto-switch off: asks y/N before moving"; fi; }

_ai_failover() {
  case ${1:-} in
    '') _fo_ui ;;
    on) has jq || { _fo_card "✻ Failover" "Failover needs jq (brew install jq / apt install jq)"; return 1; }
        # shortcut: native Windows is off (the mod's kill, Windows transcript paths and the 32 KB
        # command line are unverified there); WSL works like Linux.
        is_windows && { _fo_card "✻ Failover" "Not supported on native Windows yet. Use WSL."; return 1; }
        mkdir -p "$(dirname "$AI_FO_ORDER")" && : > "$AI_FO_ORDER.on" && _fo_card "✻ Failover" "Failover is $(_fo_on_text)" "$(_fo_auto_text)" ;;
    off) rm -f "$AI_FO_ORDER.on"; _fo_card "✻ Failover" "Failover is $(_fo_on_text)" ;;
    ls) _fo_show ;;
    auto) case ${2:-} in
            on) mkdir -p "$(dirname "$AI_FO_ORDER")" && : > "$AI_FO_ORDER.auto" ;;
            off) rm -f "$AI_FO_ORDER.auto" ;;
            '') ;;
            *) _fo_card "✻ Failover" "ai failover auto [on|off]"; return 1 ;;
          esac
          _fo_card "✻ Failover" "$(_fo_auto_text)" ;;
    order) shift; _fo_list | _fo_reorder "$@" | _fo_save && _fo_show ;;
    clear) [ -z "${2:-}" ] || _fo_member_ok "$2" || { _fo_card "✻ Failover" "no member: $2"; return 1; }
           rm -rf "${AI_FO_STATE:?}/limited/${2:-}" && _fo_card "✻ Failover" "Cleared the limit of ${2:-every account}" ;;
    *) _fo_card "✻ Failover" "ai failover              priority screen" "ai failover on | off" "ai failover auto [on|off]" "ai failover ls" \
         "ai failover order <member>..." "ai failover clear [member]"; return 1 ;;
  esac
}
# a member name safe under limited/: claude/<account> or one plain name, no other '/', no '\', no leading '.'
_fo_member_ok() {
  case ${1#claude/} in ''|.*|*/*|*\\*) return 1 ;; esac
}
_fo_auto_toggle() { if _fo_auto; then _ai_failover auto off; else _ai_failover auto on; fi; }
_fo_toggle() { if _fo_on; then _ai_failover off; else _ai_failover on; fi; }
_fo_on_text() { if _fo_on; then printf '\033[1;32mon\033[0m'; else printf '\033[2moff\033[0m'; fi; }

# ── screens ────────────────────────────────────────────────
# a box on stderr in the picker's colors: _fo_card <title> <line>...
_fo_card() {
  local t=$1 l v w=0 c=$'\033[38;5;173m' r=$'\033[0m'; shift
  for l in "$@"; do v=$(printf '%s' "$l" | sed $'s/\033\\[[0-9;]*m//g'); [ ${#v} -gt $w ] && w=${#v}; done
  w=$((w + 4)); [ $w -ge $((${#t} + 6)) ] || w=$((${#t} + 6))
  {
    printf '\n%s╭─ \033[1m%s%s%s %s╮%s\n' "$c" "$t" "$r" "$c" "$(rep ─ $((w - ${#t} - 3)))" "$r"
    for l in "$@"; do printf '%s│%s  %s  %s│%s\n' "$c" "$r" "$(pad "$l" $((w - 4)))" "$c" "$r"; done
    printf '%s╰%s╯%s\n' "$c" "$(rep ─ $w)" "$r"
  } >&2
}
# the order as a card, for `ai failover ls|order`
_fo_show() {
  local lines=() l
  while IFS= read -r l; do [ -n "$l" ] && lines+=("$l"); done <<EOF
$(_fo_rows | cut -f3)
EOF
  _fo_card "✻ Failover: $(_fo_state)" "$(dim "top is tried first · change it: ai failover")" \
    "$(dim "$(_fo_auto_text) · ai failover auto on|off")" "" ${lines[@]+"${lines[@]}"}
}
# who a member is, for people: its label and name
_fo_label() {
  local l
  case $1 in claude/*) echo "Claude account ${1#claude/}" ;; *) l=$(_ai_get "$1" 3); echo "${l:-$1}${l:+ ($1)}" ;; esac
}
# _fo_wait: under a card. Prints go | order (o) | stop.
# Auto-switch on: counts down AI_FO_WAIT s, any key or time up = go, q = stop.
# Otherwise asks, default No: y = go, anything else (enter, end of input) = stop.
_fo_wait() {
  local k= i rc
  # drop keys typed before the prompt (a "y" meant for claude); only a terminal can hold type-ahead.
  # shortcut: bash 3.2 read -t takes whole seconds, so this costs 1 s; upgrade when 3.2 is dropped.
  if [ -t 0 ]; then while IFS= read -r -s -n 1 -t 1 k; do :; done; k=; fi
  if _fo_auto; then
    for ((i = ${AI_FO_WAIT:-5}; i > 0; i--)); do
      printf '\r\033[2m  switching in %ds · any key now · o change order · q stop \033[0m' $i >&2
      k=; IFS= read -r -s -n 1 -t 1 k; rc=$?
      case $k in q|Q) printf '\n' >&2; echo stop; return ;; o|O) printf '\n' >&2; echo order; return ;; esac
      [ $rc = 0 ] && break
    done
    printf '\n' >&2; echo go; return
  fi
  printf '\033[2m  move? y yes · o change order · N stop [y/N] \033[0m' >&2
  IFS= read -r -s -n 1 k; printf '\n' >&2
  case $k in y|Y) echo go ;; o|O) echo order ;; *) echo stop ;; esac
}
# ── members and their priority ─────────────────────────────
# every account failover can use: member TAB kind (claude|codex), in providers order
_fo_members() {
  local n a d
  for n in $(_ai_names); do
    a=$(_ai_agent "$n")
    if [ "$n" = claude ]; then
      for d in "$CLA_ROOT"/*/; do [ -d "$d" ] && printf 'claude/%s\tclaude\n' "$(basename "$d")"; done
    elif [ "$a" = claude ] || [ "$a" = codex ]; then
      printf '%s\t%s\n' "$n" "$a"
    fi
  done
}
# member TAB kind TAB on|off, in priority order: the saved order, then new members (claude first)
_fo_list() {
  _fo_members | awk -F'\t' -v OFS='\t' -v f="$AI_FO_ORDER" '
    BEGIN { while ((getline l < f) > 0) { split(l, p, "\t"); if (p[1] != "" && !(p[1] in st)) { o[++n] = p[1]; st[p[1]] = p[2] } } }
    { kind[$1] = $2; if (!($1 in st)) { if ($2 == "claude") nc[++c] = $1; else nx[++x] = $1 } }
    END {
      for (i = 1; i <= n; i++) if (o[i] in kind) print o[i], kind[o[i]], (st[o[i]] == "off" ? "off" : "on")
      for (i = 1; i <= c; i++) print nc[i], "claude", "on"
      for (i = 1; i <= x; i++) print nx[i], "codex", "on"
    }'
}
# stdin: a _fo_list. Saves order and on/off; keeps saved lines of members not here right now
# (a missing account folder), so their place and off state come back with them.
_fo_save() {
  local out; out=$(cut -f1,3)
  [ -n "$out" ] || return 0
  out=$(printf '%s\n' "$out" | awk -F'\t' -v f="$AI_FO_ORDER" '{ seen[$1] = 1; print }
    END { while ((getline l < f) > 0) { split(l, p, "\t"); if (p[1] != "" && !(p[1] in seen)) print l } }')
  mkdir -p "$(dirname "$AI_FO_ORDER")" && printf '%s\n' "$out" > "$AI_FO_ORDER"
}
# _fo_list filters: move a member up/down one place, set it on/off (toggle without a state), put names first
_fo_move() {
  awk -v m="$1" -v d="$2" '{ r[NR] = $0; split($0, f, "\t"); if (f[1] == m) at = NR }
    END { j = (d == "up") ? at - 1 : at + 1
          if (at && j >= 1 && j <= NR) { t = r[at]; r[at] = r[j]; r[j] = t }
          for (i = 1; i <= NR; i++) print r[i] }'
}
_fo_set() {
  awk -F'\t' -v OFS='\t' -v m="$1" -v s="${2:-}" '$1 == m { $3 = (s != "") ? s : ($3 == "on" ? "off" : "on") } { print }'
}
_fo_reorder() {
  awk -F'\t' -v want="$*" 'BEGIN { n = split(want, w, " ") } { r[$1] = $0; o[NR] = $1 }
    END { for (i = 1; i <= n; i++) if (w[i] in r) { print r[w[i]]; done[w[i]] = 1 } else print "no member: " w[i] > "/dev/stderr"
          for (i = 1; i <= NR; i++) if (!(o[i] in done)) print r[o[i]] }'
}
# seconds left before a member that hit its limit is tried again; fails when it is free
# shortcut: a fixed cooldown, not the account's real reset time; read error_details if that matters.
_fo_limited() {
  local f="$AI_FO_STATE/limited/$1" left
  [ -f "$f" ] || return 1
  left=$(( $(date -r "$f" +%s) + AI_FO_COOLDOWN - $(date +%s) ))
  [ $left -gt 0 ] && echo $left
}
_fo_rows() {
  local i=0 m k s l left t
  _fo_list | while IFS="$TAB" read -r m k s; do
    i=$((i + 1)); t=
    case $m in claude/*) l="claude account" ;; *) l=$(_ai_get "$m" 3) ;; esac
    left=$(_fo_limited "$m") && t=$(printf '\033[33mlimited · %dh%02dm left\033[0m' $((left / 3600)) $((left % 3600 / 60)))
    if [ "$s" = on ]; then s=$'\033[32mon \033[0m'; else s=$'\033[2moff\033[0m'; fi
    printf '%s\t%s\t\033[2m%2d\033[0m  \033[1;36m%s\033[0m %s %s %s  %s\n' \
      "$m" "$m" $i "$(pad "$m" 16)" "$(pad "$l" 18)" "$(pad "$k" 6)" "$s" "$t"
  done
}
_fo_ui() {
  has jq || { _fo_card "✻ Failover" "Failover needs jq (brew install jq / apt install jq)"; return 1; }
  local out key m rows
  while :; do
    rows=$(_fo_rows)
    [ -n "$rows" ] || { _fo_card "✻ Failover" "No Claude or Codex accounts yet." "Add one: cla add <name>  ·  ai add <name> codex"; return 1; }
    out=$(_ai_choose " ✻ Failover: $(_fo_state) · auto-switch: $(_fo_auto_word) " \
      "top = first · ctrl-u up · ctrl-d down · ctrl-x on/off · ctrl-r clear limit · ctrl-f failover on/off · ctrl-a auto-switch: $(_fo_auto_word) · esc back" \
      order "$rows") || return 0
    key=${out%%"$NL"*}; m=${out#*"$NL"}; m=${m%%"$TAB"*}
    case $key in
      up|down) _fo_list | _fo_move "$m" "$key" | _fo_save ;;
      toggle) _fo_list | _fo_set "$m" | _fo_save ;;
      clear) rm -f "$AI_FO_STATE/limited/$m" ;;
      failover) _fo_toggle 2>/dev/null || { _fo_toggle; sleep 2; } ;;
      auto) _fo_auto_toggle 2>/dev/null ;;
      *) return 0 ;;
    esac
  done
}

# ── running with failover ──────────────────────────────────
# Splits claude args into flags kept on every relaunch (FO_KEEP) and the prompt (FO_PROMPT).
# Flags that take a value come from claude --help (`--model <model>`, `--allowedTools, --allowed-tools <tools...>`).
# shortcut: a variadic flag keeps one value; pass more by repeating the flag.
_fo_split() {
  local vf a i args=("$@")
  vf=" $(claude --help 2>/dev/null | sed -nE 's/^ +(-[a-zA-Z], )?(--[a-zA-Z-]+)(, --[a-zA-Z-]+)? <.*/\1\2\3/p' | tr ',\n' '  ') "
  # claude not on PATH (the saved command is a wrapper): the common ones
  [ "$vf" != "  " ] || vf=" --model --settings --add-dir --permission-mode --mcp-config --agent --effort --name -n --append-system-prompt --system-prompt --allowedTools --allowed-tools --disallowedTools --disallowed-tools --fallback-model --output-format --input-format "
  FO_KEEP=() FO_PROMPT=
  for ((i = 0; i < ${#args[@]}; i++)); do
    a=${args[i]}
    case $a in
      --resume|-r) [ "${args[i+1]:--}" = "${args[i+1]#-}" ] && i=$((i + 1)); continue ;;
      --session-id) i=$((i + 1)); continue ;;
      --continue|-c|--resume=*|--session-id=*) continue ;;
      --) FO_PROMPT="${args[*]:i+1}"; break ;;
      -*) FO_KEEP+=("$a")
          case "$vf" in *" ${a%%=*} "*) [ "$a" = "${a%%=*}" ] && [ $((i + 1)) -lt ${#args[@]} ] && { i=$((i + 1)); FO_KEEP+=("${args[i]}"); } ;; esac ;;
      *) FO_PROMPT=$a ;;
    esac
  done
}
# the config folder a claude member logs in with
_fo_claude_dir() {
  local d
  case $1 in
    claude/*) echo "$CLA_ROOT/${1#claude/}" ;;
    *) d=$(_ai_get "$1" 2 | sed -n 's/.*CLAUDE_CONFIG_DIR=\([^ ]*\).*/\1/p'); d=${d/#\~/$HOME}; echo "${d:-${FO_BASE_CFG:-$HOME/.claude}}" ;;
  esac
}
_fo_launch_claude() {
  local m=$1 c; shift
  case $m in
    claude/*) _cla_trust_sync "$CLA_ROOT/${m#claude/}"
              CLAUDE_CONFIG_DIR=$(native_path "$CLA_ROOT/${m#claude/}"); export CLAUDE_CONFIG_DIR
              c=$(_ai_cmd claude claude) ;;
    *) # the config folder ai was started with, not one a claude/<account> before it set
       if [ -n "${FO_BASE_CFG:-}" ]; then CLAUDE_CONFIG_DIR=$FO_BASE_CFG; export CLAUDE_CONFIG_DIR; else unset CLAUDE_CONFIG_DIR; fi
       c=$(_ai_cmd "$m" claude) ;;
  esac
  AI_FO_MEMBER=$(_fo_label "$m") _ai_exec "$c --plugin-dir $(printf %q "$(native_path "$AI_FO_MOD")")" "$@"
}
_fo_launch_codex() {
  local m=$1 d; shift
  if d=$(_ai_account_dir "$m"); then CODEX_HOME=$(native_path "$d"); export CODEX_HOME
  # a CODEX_HOME inherited from an ai codex account is not the main login
  elif [ -n "${CODEX_HOME:-}" ] && _fo_under "$CODEX_HOME" "$AI_ACCOUNTS"; then unset CODEX_HOME; fi
  _ai_exec "$(_ai_cmd "$m" codex)" "$@"
}
# markdown of a Claude transcript's main conversation, its last 60 KB (one argument must fit)
_fo_handoff() {
  jq -r 'select(.type == "user" or .type == "assistant")
    | select(.isMeta != true and .isApiErrorMessage != true and .isSidechain != true)
    | ((.message.content // "") | if type == "string" then . else map(select(.type == "text") | .text) | join("\n") end) as $t
    | select($t != "") | "### \(.type)\n\($t)\n"' "$1" > "$AI_FO_STATE/handoff.$$.md" || return 1
  # cut to the last 60 KB; sed drops the line the cut split
  if [ "$(wc -c < "$AI_FO_STATE/handoff.$$.md")" -gt 60000 ]; then tail -c 60000 "$AI_FO_STATE/handoff.$$.md" | sed 1d
  else cat "$AI_FO_STATE/handoff.$$.md"; fi
  rm -f "$AI_FO_STATE/handoff.$$.md"
}
# next member to try: on, not tried in this run ($1 = " a b "), not cooling down
_fo_next() {
  _fo_list | {
    while IFS="$TAB" read -r m k s; do
      [ "$s" = on ] || continue
      case $1 in *" $m "*) continue ;; esac
      _fo_limited "$m" >/dev/null && continue
      echo "$m"; exit 0
    done; exit 1; }
}
_fo_kind() { _fo_list | awk -F'\t' -v m="$1" '$1 == m { print $2; exit }'; }

# _fo_run <member> [claude args]: start on <member>, move on at each limit.
_fo_run() {
  local m=$1 kind tried=" " sid= tr= rc dest first=1 history run k
  shift
  FO_BASE_CFG=${CLAUDE_CONFIG_DIR:-}
  # inside a cla session the inherited folder is that account, not main (~/.claude)
  [ -n "$FO_BASE_CFG" ] && _fo_under "$FO_BASE_CFG" "$CLA_ROOT" && FO_BASE_CFG=
  _fo_split "$@"
  mkdir -p "$AI_FO_STATE/limited"
  AI_FO_HANDOFF="$AI_FO_STATE/handoff.$$.json"; export AI_FO_HANDOFF
  while :; do
    kind=$(_fo_kind "$m"); tried="$tried$m "
    rm -f "$AI_FO_HANDOFF"
    if [ "$kind" = codex ]; then
      if [ -n "$tr" ]; then
        history=$(_fo_handoff "$tr") && [ -n "$history" ] || { _fo_card "✻ Failover" "Could not write a handoff from" "$tr"; return 1; }
        set -- "$AI_FO_PROMPT Below is the end of the Claude Code transcript (full file: $tr).

$history"
      else
        set -- ${FO_PROMPT:+"$FO_PROMPT"}
      fi
      unset AI_FO_HANDOFF
      _fo_launch_codex "$m" "$@"; return
    fi
    if [ $first = 1 ] || [ -z "$sid" ]; then
      run=("$@")
    else
      # Sessions live per config folder. The <id>/ folder beside the transcript holds its subagent and tool files.
      dest=$(_fo_claude_dir "$m")/projects/$(basename "$(dirname "$tr")")
      mkdir -p "$dest" && { [ "$tr" -ef "$dest/$(basename "$tr")" ] || {
        cp -p "$tr" "$dest/" && { [ ! -d "${tr%.jsonl}" ] || cp -Rp "${tr%.jsonl}" "$dest/"; }; }; } ||
        { _fo_card "✻ Failover" "Could not copy the session to $(_fo_label "$m")" "$tr"; return 1; }
      run=(${FO_KEEP[@]+"${FO_KEEP[@]}"} --resume "$sid" "$AI_FO_PROMPT")
    fi
    first=0
    AI_FO_NEXT=$(_fo_next "$tried"); AI_FO_NEXT=${AI_FO_NEXT:+$(_fo_label "$AI_FO_NEXT")}; export AI_FO_NEXT
    _fo_launch_claude "$m" ${run[@]+"${run[@]}"}; rc=$?
    [ -f "$AI_FO_HANDOFF" ] || return $rc   # ended normally: no limit hit
    mkdir -p "$(dirname "$AI_FO_STATE/limited/$m")" && touch "$AI_FO_STATE/limited/$m"
    sid=$(jq -r '.session_id // empty' "$AI_FO_HANDOFF"); tr=$(jq -r '.transcript_path // empty' "$AI_FO_HANDOFF")
    [ -f "$tr" ] || { sid=; tr=; }
    rm -f "$AI_FO_HANDOFF"
    local prev=$m k
    printf '\033[H\033[2J' >&2   # a clean screen for the switch, not claude's exit text
    while :; do
      if m=$(_fo_next "$tried"); then
        k=; [ "$(_fo_kind "$m")" = codex ] && k="This sends the conversation to OpenAI (Codex)."
        _fo_card "✻ Failover" "$(_fo_label "$prev") hit its usage limit." \
          "→ Move ${sid:+this session }to $(_fo_label "$m")?" ${k:+"$k"}
        case $(_fo_wait) in go) break ;; stop) return 1 ;; esac
      else
        _fo_card "✻ Failover" "$(_fo_label "$prev") hit its usage limit." "No other account is on and free right now."
        printf '\033[2m  o change order · any other key stops \033[0m' >&2
        k=; IFS= read -r -s -n 1 k; printf '\n' >&2
        case $k in o|O) ;; *) return 1 ;; esac
      fi
      _fo_ui; _fo_on || return 1
    done
  done
}
