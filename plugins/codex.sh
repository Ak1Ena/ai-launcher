# shellcheck shell=bash
# OpenAI Codex CLI. Usage comes from the rate limits saved in the newest session log,
# so it is as fresh as your last codex run. Also used for every provider that runs codex,
# reading its own login folder if it has one.
_ai_codex_rows() {
  has codex || return 0
  local n=${1:-codex} home f rl= a ap ar b bp br plan
  home=$(_ai_account_dir "$n") || home=${CODEX_HOME:-$HOME/.codex}
  # session files live in YYYY/MM/DD/rollout-<time>.jsonl, so name order = time order
  f=$(find "$home/sessions" -name '*.jsonl' -type f 2>/dev/null | sort | tail -1)
  [ -n "$f" ] && rl=$(grep '"rate_limits"' "$f" | tail -1 | jq -c '[.. | .rate_limits? // empty][-1]' 2>/dev/null)
  [ -n "$rl" ] || rl='{}'
  IFS='|' read -r a ap ar b bp br plan <<EOF
$(printf '%s' "$rl" | jq -r '
    def w: if . == null then "", "", "" else
      (.window_minutes | if . % 1440 == 0 then "\(./1440)d" elif . % 60 == 0 then "\(./60)h" else "\(.)m" end),
      (.used_percent | floor | tostring), (.resets_at | localtime | strftime("%a %H:%M")) end;
    [(.primary | w), (.secondary | w), (.plan_type // "")] | join("|")' 2>/dev/null)
EOF
  _ai_row "$n" "$n" "$n" "$(_ai_get "$n" 3)${plan:+ · $plan}" "$(_ai_bar "${a:-5h}" "$ap" "$ar")  $(_ai_bar "$b" "$bp" "$br")"
}
