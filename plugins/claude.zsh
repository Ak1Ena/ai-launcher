# Claude Code, one row per account in ~/.claude-accounts, with live 5h / weekly usage.
#   cla                 pick account (with usage) and run
#   cla-team [name]     same, team mode (teammates in tmux split panes)
#   cla add <name>      add account and log in
#   cla ls              list accounts + who is logged in
#   cla rm <name>       remove account
#   cla mv <old> <new>  rename account (keeps login)
#   cla <name> [team] [claude args]
zmodload zsh/datetime  # EPOCHSECONDS
CLA_ROOT=${CLA_ROOT:-$HOME/.claude-accounts}

_ai_claude_rows() { for d in $CLA_ROOT/*(N/); do _cla_usage $d & done | sort; }
_ai_claude_run() { local a=$1; shift; cla $a ${AI_TEAM:+team} "$@"; }

cla() {
  local root=$CLA_ROOT
  case "$1" in
    "")
      local pick=$(_cla_pick) || return
      cla "$pick" ;;
    ls)
      for d in $root/*(N/); do echo "${d:t}: $(CLAUDE_CONFIG_DIR=$d claude auth status 2>&1 | grep -iom1 '[^ ]*@[^ ",]*' || echo 'not logged in')"; done
      return ;;
    add)
      [[ -z "$2" ]] && { echo "usage: cla add <name>"; return 1; }
      _cla_init $root/$2 && CLAUDE_CONFIG_DIR=$root/$2 claude auth login ;;
    rm)
      [[ -d $root/$2 && -n "$2" ]] || { echo "no account: $2"; return 1; }
      CLAUDE_CONFIG_DIR=$root/$2 claude auth logout; rm -rf $root/$2 && echo "removed $2" ;;
    mv)
      [[ -n "$2" && -n "$3" && -d $root/$2 ]] || { echo "usage: cla mv <old> <new>"; return 1; }
      [[ -e $root/$3 ]] && { echo "'$3' already exists"; return 1; }
      local old new acct secret
      # macOS: login lives in Keychain under a name derived from the folder path, so move it too.
      # Linux/WSL: login is .credentials.json inside the folder, so the mv below carries it.
      if (( $+commands[security] )); then
        old=$(_cla_svc $root/$2) new=$(_cla_svc $root/$3)
        acct=$(security find-generic-password -s "$old" 2>/dev/null | sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')
        secret=$(security find-generic-password -s "$old" -w 2>/dev/null)
      fi
      mv $root/$2 $root/$3 || return 1
      if [[ -n $secret ]]; then
        security add-generic-password -U -s "$new" -a "$acct" -w "$secret" && security delete-generic-password -s "$old" >/dev/null 2>&1
      fi
      echo "renamed $2 -> $3" ;;
    *)
      [[ -d $root/$1 ]] || { echo "no account '$1'. run: cla add $1"; return 1; }
      local dir=$root/$1; shift
      _ai_trust_share $PWD; _cla_trust_sync $dir
      if [[ $1 == team ]]; then shift; CLAUDE_CONFIG_DIR=$dir _cla_team "$@"
      else CLAUDE_CONFIG_DIR=$dir _ai_exec "$(_ai_cmd claude claude)" "$@"; fi ;;
  esac
}
cla-team() {
  if [[ -n "$1" && $1 != -* ]]; then local a=$1; shift; cla "$a" team "$@"
  else local pick=$(_cla_pick) || return; cla "$pick" team "$@"; fi
}
# team mode: teammates in tmux split panes. Outside tmux, opens/attaches session "claude-<folder>-<account>".
_cla_team() {
  local -a cmd=(env CLAUDE_MODE=team ${CLAUDE_CONFIG_DIR:+CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR} $(_ai_awake)
                ${(Q)${(z)$(_ai_cmd claude claude)}} --settings '{"teammateMode":"tmux"}' "$@")
  if [[ -n "$TMUX" ]]; then
    "${cmd[@]}"; tmux set -pu @claude_mode 2>/dev/null  # clear status badge on exit
  else
    local s="claude-${${PWD:t}//[.:]/-}${CLAUDE_CONFIG_DIR:+-${CLAUDE_CONFIG_DIR:t}}"
    tmux new-session -A -s "$s" -c "$PWD" "${(j: :)${(@q)cmd}}"
  fi
}
_cla_pick() {
  _ai_claude_rows | sort | _ai_fzf ' ✻ Claude accounts ' \
    "$(printf '%-9s %-26s  %-34s%s' account email '5-hour limit' 'weekly limit')" | cut -f2 | grep .
}
# one row per account: name, email, 5h / 7d plan usage.
# Usage comes from <account>/usage/usage.json, which the status line script saves on every
# refresh (scripts/statusline-usage.sh), so it is as fresh as that
# account's last session. A window whose reset time has passed shows 0%.
_cla_usage() {
  local d=$1 f fr s sr saved email note age
  email=$(jq -r '.oauthAccount.emailAddress // empty' $d/.claude.json 2>/dev/null)
  IFS="|" read -r f fr s sr saved < <(jq -r --argjson now $EPOCHSECONDS '
    def t: .resets_at | localtime | strftime("%a %H:%M");
    def v: if . == null or .used_percentage == null then "", ""
           elif (.resets_at // 0) < $now then "0", "" else (.used_percentage|floor|tostring), t end;
    [(.rate_limits.five_hour|v), (.rate_limits.seven_day|v), (.saved_at|tostring)] | join("|")' \
    $d/usage/usage.json 2>/dev/null)
  if [[ -n $saved ]]; then
    age=$(( (EPOCHSECONDS - saved) / 60 ))
    (( age >= 2880 )) && note="$(( age / 1440 ))d" || { (( age >= 120 )) && note="$(( age / 60 ))h" || note="${age}m"; }
    (( age >= 10 )) && note=$'\e[2m · '"$note old"$'\e[0m' || note=""
  fi
  [[ -n $email ]] || email=$'\e[31mnot logged in             \e[0m'  # padded: escapes break %-26s
  _ai_row claude ${d:t} ${d:t} "$email" "$(_ai_bar 5h "$f" "$fr")  $(_ai_bar 7d "$s" "$sr")$note"
}
# Claude Code saves "trust this folder" per config dir, so each account would ask again for
# folders already trusted elsewhere. Copy the trusted flag from the main config and every
# account into this one before launch. Only that flag is touched, and only when something is missing.
_cla_trust_sync() {
  local dst=$1/.claude.json tmp
  local -a src=($HOME/.claude.json $CLA_ROOT/*/.claude.json(N))
  [[ -r $dst ]] || return 0
  local trusted=$(jq -cs '[.[] | .projects // {} | to_entries[] | select(.value.hasTrustDialogAccepted == true) | .key] | unique' $src 2>/dev/null)
  [[ -n $trusted ]] || return 0
  jq -e --argjson t "$trusted" '(.projects // {}) as $p | any($t[]; $p[.].hasTrustDialogAccepted != true)' $dst >/dev/null 2>&1 || return 0
  tmp=$(mktemp) || return 0
  if jq --argjson t "$trusted" '.projects = reduce $t[] as $k (.projects // {}; .[$k].hasTrustDialogAccepted = true)' $dst > $tmp && [[ -s $tmp ]]; then
    cat $tmp > $dst  # keep the file's permissions (600)
  fi
  rm -f $tmp
}
_cla_svc() { echo "Claude Code-credentials-$(printf %s "$1" | shasum -a 256 | cut -c1-8)"; }
_cla_init() {
  mkdir -p "$1"
  # share settings, skills, plugins, memory with main account; login stays separate
  for f in settings.json settings.local.json CLAUDE.md agents skills plugins hooks commands projects; do
    [[ -e ~/.claude/$f && ! -e $1/$f ]] && ln -s ~/.claude/$f "$1/$f"
  done
}
