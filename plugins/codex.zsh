# OpenAI Codex CLI. Usage comes from the rate limits saved in the newest session log,
# so it is as fresh as your last codex run.
_ai_codex_run() { shift; _ai_exec "$(_ai_cmd codex codex)" "$@"; }
_ai_codex_rows() {
  (( $+commands[codex] )) || return
  local -a logs=(${CODEX_HOME:-$HOME/.codex}/sessions/**/*.jsonl(N.om[1]))
  local rl a ap ar b bp br plan
  [[ -n $logs ]] && rl=$(grep '"rate_limits"' $logs[1] | tail -1 | jq -c '[.. | .rate_limits? // empty][-1]' 2>/dev/null)
  IFS="|" read -r a ap ar b bp br plan < <(jq -r '
    def w: if . == null then "", "", "" else
      (.window_minutes | if . % 1440 == 0 then "\(./1440)d" elif . % 60 == 0 then "\(./60)h" else "\(.)m" end),
      (.used_percent|floor|tostring), (.resets_at|localtime|strftime("%a %H:%M")) end;
    [(.primary|w), (.secondary|w), (.plan_type // "")] | join("|")' <<<"${rl:-{}}" 2>/dev/null)
  _ai_row codex codex codex "${plan:+$plan plan · }last run" "$(_ai_bar "${a:-5h}" "$ap" "$ar")  $(_ai_bar "$b" "$bp" "$br")"
}
