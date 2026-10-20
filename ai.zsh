# ai: one command for every coding agent.
#   ai [args]                         pick a provider/account (live usage shown), launch in current dir
#                                     keys: ctrl-t claude team mode · ctrl-a add · ctrl-e edit · ctrl-x delete
#   ai ls                             list providers
#   ai add  [name] [command] [label]  add a provider (asks for anything left out)
#   ai edit <name> [command] [label]  change its launch command / label (asks if left out)
#   ai rm   <name>                    delete a provider
# Providers live in ~/.config/ai-launcher/providers (name TAB command TAB label).
# A provider with a plugins/<name>.zsh file gets usage bars; any other just launches.

AI_HOME=${${(%):-%x}:A:h}
AI_PROVIDERS=${AI_PROVIDERS:-${XDG_CONFIG_HOME:-$HOME/.config}/ai-launcher/providers}
# seeded on first run, only the ones installed
typeset -gA AI_DEFAULTS=(claude "Claude Code" codex "Codex CLI" gemini "Gemini CLI" agy "Antigravity"
                         kimi "Kimi Code" opencode "OpenCode" qwen "Qwen Code")
AI_DEFAULT_ORDER=(claude codex gemini agy kimi opencode qwen)

ai() {
  case $1 in ls|add|edit|rm) local sub=$1; shift; _ai_$sub "$@"; return ;; esac
  local out key row name id
  while true; do
    [[ -n $(_ai_names) ]] || { echo "no providers. add one: ai add <name> [command]"; return 1; }
    out=$(_ai_rows | _ai_fzf ' ✻ AI agents ' \
      'enter launch · ctrl-t claude team · ctrl-a add · ctrl-e edit · ctrl-x delete' \
      --expect=ctrl-t,ctrl-a,ctrl-e,ctrl-x) || return
    key=${out%%$'\n'*}; row=${out#*$'\n'}
    name=${row%%$'\t'*}; id=${${row#*$'\t'}%%$'\t'*}
    case $key in
      ctrl-a) _ai_add ;;
      ctrl-e) _ai_edit $name ;;
      ctrl-x) _ai_rm $name ;;
      *) local team=; [[ $key == ctrl-t ]] && team=1
         AI_TEAM=$team _ai_run $name $id "$@"; return ;;
    esac
  done
}

# ── providers file ──────────────────────────────────────
_ai_seed() {
  [[ -e $AI_PROVIDERS ]] && return
  mkdir -p ${AI_PROVIDERS:h}
  for n in $AI_DEFAULT_ORDER; do (( $+commands[$n] )) && print -r -- "$n"$'\t'"$n"$'\t'"$AI_DEFAULTS[$n]"; done > $AI_PROVIDERS
}
_ai_names() { _ai_seed; cut -f1 $AI_PROVIDERS; }
# _ai_get <name> <field: 2 = command, 3 = label>
_ai_get() { _ai_seed; awk -F'\t' -v k="$1" -v f="$2" '$1==k {print $f; exit}' $AI_PROVIDERS; }
_ai_put() {  # _ai_put <name> [command label]; no command = delete. keeps the row's position
  local out=$(K=$1 C=$2 L=$3 awk -F'\t' -v OFS='\t' '
    $1 == ENVIRON["K"] { if (ENVIRON["C"] != "") print ENVIRON["K"], ENVIRON["C"], ENVIRON["L"]; done = 1; next }
    { print } END { if (!done && ENVIRON["C"] != "") print ENVIRON["K"], ENVIRON["C"], ENVIRON["L"] }' $AI_PROVIDERS)
  print -r -- "$out" | grep . > $AI_PROVIDERS
}
# launch command: saved one, else the plain name (used by cla even if claude was deleted)
_ai_cmd() { local c=$(_ai_get $1 2); print -r -- "${c:-$2}"; }

# ── subcommands ─────────────────────────────────────────
_ai_ls() {
  _ai_seed
  while IFS=$'\t' read -r n c l; do
    printf '\e[1;36m%-9s\e[0m %-34s \e[2m%s%s\e[0m\n' $n "$c" "$l" "${functions[_ai_${n}_rows]:+  · usage}"
  done < $AI_PROVIDERS
}
_ai_add() {
  _ai_seed
  local n=$1 c=$2 l=$3
  [[ -n $n ]] || vared -p 'name: ' n
  [[ -n $n && $n != *[[:space:]]* ]] || { echo "name must be one word"; return 1; }
  [[ -n $(_ai_get $n 1) ]] && { echo "'$n' exists. change it: ai edit $n"; return 1; }
  [[ -n $c ]] || { c=$n; vared -p 'command: ' c; }
  [[ -n $l ]] || { l=${AI_DEFAULTS[$n]:-$n}; vared -p 'label: ' l; }
  [[ -n $c ]] || { echo "command is required"; return 1; }
  _ai_put $n "$c" "$l" && echo "added $n -> $c"
}
_ai_edit() {
  local n=$1 c=$2 l=$3
  [[ -n $n && -n $(_ai_get $n 1) ]] || { echo "no provider '$n'. see: ai ls"; return 1; }
  [[ -n $c ]] || { c=$(_ai_get $n 2); vared -p 'command: ' c; }
  [[ -n $l ]] || { l=$(_ai_get $n 3); vared -p 'label: ' l; }
  [[ -n $c ]] || { echo "command is required"; return 1; }
  _ai_put $n "$c" "$l" && echo "saved $n -> $c"
}
_ai_rm() {
  local n=$1
  [[ -n $n && -n $(_ai_get $n 1) ]] || { echo "no provider '$n'. see: ai ls"; return 1; }
  read -q "?delete provider '$n'? [y/N] " || { echo; return 1; }
  echo; _ai_put $n && echo "deleted $n (bring back: ai add $n)"
}

# ── picker + launch ─────────────────────────────────────
# rows from every provider, fetched in parallel, kept in providers-file order
_ai_rows() {
  local i=0
  for n in $(_ai_names); do
    (( i++ ))
    if (( $+functions[_ai_${n}_rows] )); then _ai_${n}_rows | sed "s/^/$i\t/" &
    else _ai_row $n $n $n "$(_ai_get $n 3)" $'\e[2mno usage data\e[0m' | sed "s/^/$i\t/"; fi
  done | sort -n -s -k1,1 | cut -f2-
}
_ai_run() {
  local n=$1 id=$2; shift 2
  [[ $n == claude ]] || _ai_trust_share $PWD  # claude rows go through cla, which shares too
  if (( $+functions[_ai_${n}_run] )); then _ai_${n}_run $id "$@"
  else _ai_exec "$(_ai_cmd $n $n)" "$@"; fi
}
# run a saved command string plus extra args
_ai_exec() { local -a cmd=(${(Q)${(z)1}}); shift; $(_ai_awake) "${cmd[@]}" "$@"; }
# keep the machine awake while an agent runs: macOS only, nothing elsewhere (Linux, WSL)
_ai_awake() { (( $+commands[caffeinate] )) && print -l caffeinate -i; }
# one picker row: <provider> TAB <id> TAB <shown text>
_ai_row() { printf '%s\t%s\t\e[1;36m%-9s\e[0m %-26s  %s\n' "$1" "$2" "$3" "$4" "$5"; }
# "5h ███░░░░░░░  33% ↻ Tue 12:00"; green < 60 <= yellow < 90 <= red
_ai_bar() {
  local label=$1 pct=$2 at=$3 c n
  [[ -z $label ]] && { printf '%33s' ''; return; }
  [[ -z $pct ]] && { printf '%-3s \e[2m%-29s\e[0m' $label '──────────  n/a'; return; }
  (( pct >= 90 )) && c=31 || { (( pct >= 60 )) && c=33 || c=32; }
  n=$(( (pct + 5) / 10 )); (( n > 10 )) && n=10
  printf '%-3s \e[%sm%s\e[2m%s\e[0m \e[1;%sm%3d%%\e[0m \e[2m%-11s\e[0m' $label $c "${(l:n::█:)}" "${(l:10-n::░:)}" $c $pct "${at:+↻ $at}"
}
_ai_fzf() {
  local label=$1 hint=$2; shift 2
  fzf --ansi --delimiter=$'\t' --with-nth=3 --height=~16 --reverse --border=rounded \
      --border-label="$label" --prompt='  ' --pointer='▶' --info=hidden --no-scrollbar \
      --header=$'\e[2m'"$hint"$'\e[0m' \
      --color='border:#d97757,label:#d97757:bold,pointer:#d97757,prompt:#d97757,hl:#d97757,hl+:#d97757' "$@"
}

source $AI_HOME/trust.zsh
for _f in $AI_HOME/plugins/*.zsh(N); do source $_f; done; unset _f
