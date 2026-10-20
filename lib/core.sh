# shellcheck shell=bash
# ai-launcher core. Plain bash 3.2+ (the bash built into macOS), so nothing has to be installed.
# Optional: fzf (nicer picker), jq (usage bars + shared trust), tmux (Claude team mode).

AI_PROVIDERS=${AI_PROVIDERS:-${XDG_CONFIG_HOME:-$HOME/.config}/ai-launcher/providers}
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

ai_main() {
  case ${1:-} in
    ls|add|edit|rm) local sub=$1; shift; "_ai_$sub" "$@"; return ;;
    cla) shift; cla_main "$@"; return ;;
    cla-team) shift; cla_team_main "$@"; return ;;
    help|-h|--help) _ai_help; return ;;
  esac
  local out key row name id
  while :; do
    _ai_seed
    [ -s "$AI_PROVIDERS" ] || { echo "no providers. add one: ai add <name> [command]"; return 1; }
    out=$(_ai_choose ' ✻ AI agents ' 'enter launch · ctrl-t claude team · ctrl-a add · ctrl-e edit · ctrl-x delete' keys "$(_ai_rows)") || return 0
    key=${out%%"$NL"*}; row=${out#*"$NL"}
    name=${row%%"$TAB"*}; id=${row#*"$TAB"}; id=${id%%"$TAB"*}
    case $key in
      add) _ai_add ;;
      edit) _ai_edit "$name" ;;
      delete) _ai_rm "$name" ;;
      *) [ "$key" = team ] && export AI_TEAM=1
         _ai_run "$name" "$id" "$@"; return ;;
    esac
  done
}

_ai_help() {
  cat <<'EOF'
ai                               pick an agent (with plan usage) and start it in this folder
ai [agent args]                  same, extra args go to the agent
ai ls                            list agents
ai add  [name] [command] [label] add an agent (asks for anything left out)
ai edit <name> [command] [label] change its command / label
ai rm   <name>                   delete an agent
cla [name] [team] [args]         Claude accounts: pick or start one   (cla add|ls|rm|mv)
cla-team [name] [args]           Claude team mode (needs tmux)

Picker keys: enter launch · ctrl-t team · ctrl-a add · ctrl-e edit · ctrl-x delete
Without fzf you get a numbered menu: 2 = launch, t 2 = team, a = add, e 2 = edit, x 2 = delete.
EOF
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
  case $n in ''|*[[:space:]]*) echo "name must be one word"; return 1 ;; esac
  [ -n "$(_ai_get "$n" 1)" ] && { echo "'$n' exists. change it: ai edit $n"; return 1; }
  [ -n "$c" ] || c=$(_ai_ask "command" "$n") || return 1
  [ -n "$l" ] || l=$(_ai_ask "label" "$(_ai_default_label "$n")") || return 1
  [ -n "$c" ] || { echo "command is required"; return 1; }
  _ai_put "$n" "$c" "$l" && echo "added $n -> $c"
}
_ai_edit() {
  local n=${1:-} c=${2:-} l=${3:-}
  [ -n "$n" ] && [ -n "$(_ai_get "$n" 1)" ] || { echo "no provider '$n'. see: ai ls"; return 1; }
  [ -n "$c" ] || c=$(_ai_ask "command" "$(_ai_get "$n" 2)") || return 1
  [ -n "$l" ] || l=$(_ai_ask "label" "$(_ai_get "$n" 3)") || return 1
  _ai_put "$n" "$c" "$l" && echo "saved $n -> $c"
}
_ai_rm() {
  local n=${1:-} ans
  [ -n "$n" ] && [ -n "$(_ai_get "$n" 1)" ] || { echo "no provider '$n'. see: ai ls"; return 1; }
  read -r -p "delete provider '$n'? [y/N] " ans
  case $ans in y|Y|yes) _ai_put "$n" && echo "deleted $n (bring back: ai add $n)" ;; *) return 1 ;; esac
}

# ── rows ───────────────────────────────────────────────────
# every provider's rows, fetched in parallel, kept in providers-file order
_ai_rows() {
  local i=0 n
  {
    for n in $(_ai_names); do
      i=$((i + 1))
      if is_fn "_ai_${n}_rows" && has jq; then
        _ai_"${n}"_rows | sed "s/^/$i$TAB/" &
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
# _ai_choose <title> <hint> <keys|""> <rows>. Prints "<action>\n<row>",
# action = "" (launch) | team | add | edit | delete. Fails on cancel.
# Rows come as an argument, not stdin, so the menu can read the answer from the terminal
# on stdin and draw on stderr: no /dev/tty needed (unreliable in Git Bash on Windows).
_ai_choose() {
  if has fzf; then _ai_fzf "$@"; else _ai_menu "$@"; fi
}
_ai_fzf() {
  local out key extra=()
  [ -n "${3:-}" ] && extra=(--expect=ctrl-t,ctrl-a,ctrl-e,ctrl-x)
  out=$(fzf --ansi --delimiter="$TAB" --with-nth=3 --height=~16 --reverse --border=rounded \
      --border-label="$1" --prompt='  ' --pointer='▶' --info=hidden --no-scrollbar \
      --header=$'\033[2m'"$2"$'\033[0m' \
      --color='border:#d97757,label:#d97757:bold,pointer:#d97757,prompt:#d97757,hl:#d97757,hl+:#d97757' \
      ${extra[@]+"${extra[@]}"} <<<"$4") || return 1
  [ -n "${3:-}" ] || { printf '\n%s\n' "$out"; return; }
  key=${out%%"$NL"*}
  case $key in ctrl-t) key=team ;; ctrl-a) key=add ;; ctrl-e) key=edit ;; ctrl-x) key=delete ;; esac
  printf '%s\n%s\n' "$key" "${out#*"$NL"}"
}
_ai_menu() {
  local rows=() line i=0 ans act n
  while IFS= read -r line; do [ -n "$line" ] && rows+=("$line"); done <<<"$4"
  {
    printf '\n  \033[1;38;5;173m%s\033[0m\n\n' "$1"
    for line in ${rows[@]+"${rows[@]}"}; do
      i=$((i + 1)); printf '  \033[2m%2d\033[0m  %s\n' $i "${line#*"$TAB"*"$TAB"}"
    done
    if [ -n "${3:-}" ]; then dim "  number = launch · t N = claude team · a = add · e N = edit · x N = delete · q = quit"
    else dim "  number = choose · q = quit"; fi
    printf '\n'
  } >&2
  read -r -p "  › " ans || return 1
  set -- $ans
  case ${1:-q} in
    q) return 1 ;;
    a) printf 'add\n\n'; return ;;
    t) act=team; n=${2:-} ;;
    e) act=edit; n=${2:-} ;;
    x) act=delete; n=${2:-} ;;
    *) act=; n=$1 ;;
  esac
  case $n in ''|*[!0-9]*) echo "not a number: $ans" >&2; return 1 ;; esac
  [ "$n" -ge 1 ] && [ "$n" -le ${#rows[@]} ] || { echo "no row $n" >&2; return 1; }
  printf '%s\n%s\n' "$act" "${rows[$((n - 1))]}"
}

# ── launch ─────────────────────────────────────────────────
_ai_run() {
  local n=$1 id=$2; shift 2
  [ "$n" = claude ] || _ai_trust_share "$PWD"  # claude rows go through cla, which shares too
  if is_fn "_ai_${n}_run"; then "_ai_${n}_run" "$id" "$@"
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
