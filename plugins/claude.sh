# shellcheck shell=bash
# Claude Code: one row per account in $CLA_ROOT (default ~/.claude-accounts), with 5h / weekly usage.
#   cla                 pick an account and start it
#   cla-team [name]     same, team mode (teammates in tmux split panes)
#   cla add <name>      add an account and log in
#   cla ls              list accounts and who is logged in
#   cla rm <name>       remove an account
#   cla mv <old> <new>  rename an account (keeps login)
#   cla <name> [team] [claude args]
CLA_ROOT=${CLA_ROOT:-$HOME/.claude-accounts}

_ai_claude_rows() {
  local d
  { for d in "$CLA_ROOT"/*/; do [ -d "$d" ] && _cla_usage "${d%/}" & done; wait; } | sort
}
_ai_claude_run() {
  local a=$1; shift
  if [ -n "${AI_TEAM:-}" ]; then cla_main "$a" team "$@"; else cla_main "$a" "$@"; fi
}

cla_main() {
  local root=$CLA_ROOT dir
  case ${1:-} in
    "")
      local pick; pick=$(_cla_pick) || return 0
      cla_main "$pick" ;;
    ls)
      for dir in "$root"/*/; do
        [ -d "$dir" ] || continue; dir=${dir%/}
        echo "${dir##*/}: $(CLAUDE_CONFIG_DIR=$dir claude auth status 2>&1 | grep -iom1 '[^ ]*@[^ ",]*' || echo 'not logged in')"
      done ;;
    add)
      [ -n "${2:-}" ] || { echo "usage: cla add <name>"; return 1; }
      _cla_init "$root/$2" && CLAUDE_CONFIG_DIR=$root/$2 claude auth login ;;
    rm)
      [ -n "${2:-}" ] && [ -d "$root/$2" ] || { echo "no account: ${2:-}"; return 1; }
      CLAUDE_CONFIG_DIR=$root/$2 claude auth logout; rm -rf "${root:?}/$2" && echo "removed $2" ;;
    mv)
      [ -n "${2:-}" ] && [ -n "${3:-}" ] && [ -d "$root/$2" ] || { echo "usage: cla mv <old> <new>"; return 1; }
      [ -e "$root/$3" ] && { echo "'$3' already exists"; return 1; }
      local old new acct secret=
      # macOS: the login lives in Keychain under a name made from the folder path, so move it too.
      # Linux/WSL: the login is .credentials.json inside the folder, so the mv below carries it.
      if has security; then
        old=$(_cla_svc "$root/$2"); new=$(_cla_svc "$root/$3")
        acct=$(security find-generic-password -s "$old" 2>/dev/null | sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')
        secret=$(security find-generic-password -s "$old" -w 2>/dev/null)
      fi
      mv "$root/$2" "$root/$3" || return 1
      if [ -n "$secret" ]; then
        security add-generic-password -U -s "$new" -a "$acct" -w "$secret" &&
          security delete-generic-password -s "$old" >/dev/null 2>&1
      fi
      echo "renamed $2 -> $3" ;;
    *)
      [ -d "$root/$1" ] || { echo "no account '$1'. run: cla add $1"; return 1; }
      dir=$root/$1; shift
      _ai_trust_share "$PWD"; _cla_trust_sync "$dir"
      export CLAUDE_CONFIG_DIR=$dir
      if [ "${1:-}" = team ]; then shift; _cla_team "$@"
      else _ai_exec "$(_ai_cmd claude claude)" "$@"; fi ;;
  esac
}
cla_team_main() {
  case ${1:-} in
    ''|-*) local pick; pick=$(_cla_pick) || return 0; cla_main "$pick" team "$@" ;;
    *) local a=$1; shift; cla_main "$a" team "$@" ;;
  esac
}
# team mode: teammates in tmux split panes. Outside tmux, opens/attaches session "claude-<folder>-<account>".
_cla_team() {
  has tmux || { echo "team mode needs tmux (brew install tmux / apt install tmux)"; return 1; }
  local cmd s
  cmd="CLAUDE_MODE=team CLAUDE_CONFIG_DIR=$(printf %q "$CLAUDE_CONFIG_DIR") "
  has caffeinate && cmd="${cmd}caffeinate -i "
  cmd="$cmd$(_ai_cmd claude claude) --settings '{\"teammateMode\":\"tmux\"}'"
  [ $# -gt 0 ] && cmd="$cmd$(printf ' %q' "$@")"
  if [ -n "${TMUX:-}" ]; then
    bash -c "$cmd"; tmux set -pu @claude_mode 2>/dev/null  # clear status badge on exit
  else
    s="claude-$(basename "$PWD" | tr '.:' '--')-$(basename "$CLAUDE_CONFIG_DIR")"
    tmux new-session -A -s "$s" -c "$PWD" "bash -c $(printf %q "$cmd")"
  fi
}
_cla_pick() {
  local out
  out=$(_ai_claude_rows | _ai_choose ' ✻ Claude accounts ' \
    "$(pad account 9) $(pad email 26)  $(pad '5-hour limit' 34)weekly limit") || return 1
  out=${out#*"$NL"}; out=${out#*"$TAB"}; printf '%s\n' "${out%%"$TAB"*}"
}

# one row per account: name, email, 5h / 7d plan usage.
# Usage comes from <account>/usage/usage.json, which scripts/statusline-usage.sh saves on every
# status line refresh, so it is as fresh as that account's last session.
# A window whose reset time has passed shows 0%.
_cla_usage() {
  local d=$1 email f fr s sr saved note= age now
  now=$(date +%s)
  email=$(jq -r '.oauthAccount.emailAddress // empty' "$d/.claude.json" 2>/dev/null)
  IFS='|' read -r f fr s sr saved <<EOF
$(jq -r --argjson now "$now" '
    def t: .resets_at | localtime | strftime("%a %H:%M");
    def v: if . == null or .used_percentage == null then "", ""
           elif (.resets_at // 0) < $now then "0", "" else (.used_percentage | floor | tostring), t end;
    [(.rate_limits.five_hour | v), (.rate_limits.seven_day | v), (.saved_at | tostring)] | join("|")' \
    "$d/usage/usage.json" 2>/dev/null)
EOF
  if [ -n "$saved" ] && [ "$saved" != null ]; then
    age=$(( (now - saved) / 60 ))
    if [ $age -ge 2880 ]; then note="$((age / 1440))d"; elif [ $age -ge 120 ]; then note="$((age / 60))h"; else note="${age}m"; fi
    if [ $age -ge 10 ]; then note=$(dim " · $note old"); else note=; fi
  fi
  [ -n "$email" ] || email=$'\033[31mnot logged in\033[0m'
  _ai_row claude "${d##*/}" "${d##*/}" "$email" "$(_ai_bar 5h "$f" "$fr")  $(_ai_bar 7d "$s" "$sr")$note"
}

# Claude Code keeps "trust this folder" per config dir, so each account would ask again for
# folders already trusted elsewhere. Copy the trusted flag from the main config and every
# account into this one before launch. Only that flag is touched, and only when something is missing.
_cla_trust_sync() {
  has jq || return 0
  local dst=$1/.claude.json trusted tmp
  [ -r "$dst" ] || return 0
  trusted=$(_ai_trust_claude_files | tr '\n' '\0' | xargs -0 jq -cs \
    '[.[] | .projects // {} | to_entries[] | select(.value.hasTrustDialogAccepted == true) | .key] | unique' 2>/dev/null)
  [ -n "$trusted" ] || return 0
  jq -e --argjson t "$trusted" '(.projects // {}) as $p | any($t[]; $p[.].hasTrustDialogAccepted != true)' "$dst" >/dev/null 2>&1 || return 0
  _ai_json_edit "$dst" --argjson t "$trusted" '.projects = reduce $t[] as $k (.projects // {}; .[$k].hasTrustDialogAccepted = true)'
}
_cla_svc() { echo "Claude Code-credentials-$(printf %s "$1" | shasum -a 256 | cut -c1-8)"; }
_cla_init() {
  mkdir -p "$1"
  # share settings, skills, plugins, memory with the main account; the login stays separate
  local f
  for f in settings.json settings.local.json CLAUDE.md agents skills plugins hooks commands projects; do
    [ -e "$HOME/.claude/$f" ] && [ ! -e "$1/$f" ] && ln -s "$HOME/.claude/$f" "$1/$f"
  done
  return 0
}
