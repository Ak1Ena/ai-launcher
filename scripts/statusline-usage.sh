#!/usr/bin/env bash
# Claude Code status line command that saves plan usage for `ai` / `cla`.
# Claude Code pipes JSON with rate_limits (5-hour / weekly %, reset times) to the status line on
# every refresh. This saves it to $CLAUDE_CONFIG_DIR/usage/usage.json (default ~/.claude), one
# file per account, then pipes the same JSON to your usual status line command, if you pass one.
#
#   "statusLine": {"type": "command", "command": "bash ~/code/ai-launcher/scripts/statusline-usage.sh"}
#   "statusLine": {"type": "command", "command": "bash ~/code/ai-launcher/scripts/statusline-usage.sh my-statusline --flag"}
#
# rate_limits only exists for Pro/Max plans, after the session's first reply.
input="$(cat)"
if command -v jq >/dev/null; then
  rl="$(printf '%s' "$input" | jq -c '.rate_limits // empty' 2>/dev/null)"
  if [[ -n "$rl" && "$rl" != "null" ]]; then
    dir="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/usage"; mkdir -p "$dir"
    tmp="$dir/.usage.$$"
    jq -n --argjson rl "$rl" '{saved_at: (now|floor), rate_limits: $rl}' > "$tmp" && mv "$tmp" "$dir/usage.json"
  fi
fi
(( $# )) && printf '%s' "$input" | "$@"
exit 0
