# shellcheck shell=bash
# User plugins: folders in $AI_PLUGINS (default: `plugins` beside the providers file), each with an
# executable `plugin` that answers `rows`, `actions`, `do <action>` and `launch <id> [args]`.
# `<plugin>/.off` turns one off (never run), `<plugin>/.source` holds the git URL it came from.

_ai_plugins_dir() { printf '%s\n' "${AI_PLUGINS:-${AI_PROVIDERS%/*}/plugins}"; }
_ai_plugin_ok_name() { case $1 in ''|*[!A-Za-z0-9_-]*) return 1 ;; esac; }
_ai_plugin_ok_id() { case $1 in ''|*[!A-Za-z0-9._-]*) return 1 ;; esac; }
# installed plugins (on and off), sorted by name
_ai_plugin_names() {
  local f n
  for f in "$(_ai_plugins_dir)"/*/plugin; do
    n=${f%/plugin}; n=${n##*/}
    [ -f "$f" ] && _ai_plugin_ok_name "$n" && printf '%s\n' "$n"
  done | LC_ALL=C sort
}
# installed, runnable and on. Windows runs it through bash, so no exec bit is needed there.
_ai_plugin_ready() {
  local d; d=$(_ai_plugins_dir)/$1
  _ai_plugin_ok_name "$1" && [ -f "$d/plugin" ] && [ ! -e "$d/.off" ] && { is_windows || [ -x "$d/plugin" ]; }
}
# _ai_plugin_call <name> <secs> <args...>: run it with no tty and a watchdog. Prints its stdout
# (plus stderr for `do`). The plugin gets its own process group, so a timeout kills its children too.
_ai_plugin_call() {
  local n=$1 secs=$2 d out err rc; shift 2
  _ai_plugin_ready "$n" || return 1
  d=$(_ai_plugins_dir)/$n
  out=$(mktemp "${TMPDIR:-/tmp}/ai-plugin.XXXXXX") || return 1
  if [ "${1:-}" = do ]; then err=$out; else err=/dev/null; fi
  (
    set -m
    export AI_PLUGIN_DIR=$d
    if is_windows; then bash "$d/plugin" "$@"; else "$d/plugin" "$@"; fi </dev/null >>"$out" 2>>"$err" &
    p=$!
    # output goes to a file, so a child that outlives the kill can never hold the caller open
    ( sleep "$secs"; kill -KILL -- -"$p" || kill -KILL "$p" ) </dev/null >/dev/null 2>&1 &
    w=$!
    wait "$p"; rc=$?
    kill -- -"$w" || kill "$w"
    exit $rc
  ) 2>/dev/null; rc=$?
  cat "$out"; rm -f "$out"
  return $rc
}
# picker rows for one plugin: a header row, entries (provider field plugin:<name>), detail rows.
# Header, detail and status rows have empty provider and id, so picking them does nothing.
_ai_plugin_rows() {
  local n=$1 out line id label status title seen= first=1 hdr=
  if [ -e "$(_ai_plugins_dir)/$n/.off" ]; then _ai_row "" "" "" "$(dim "$n · off")" ""; return; fi
  out=$(_ai_plugin_call "$n" 2 rows) || { _ai_row "" "" "" "$(dim "$n: plugin failed")" ""; return; }
  out=$(printf '%s\n' "$out" | tr -d '\000-\010\013-\037\177')   # CR and control chars (TAB and newline stay)
  while IFS= read -r line; do
    if [ -n "$first" ]; then
      first=; title=$n
      case $line in "section$TAB"*) title=${line#section"$TAB"}; continue ;; esac
    fi
    case $line in
      "section$TAB"*) continue ;;
      "  $TAB"*) [ -n "$seen" ] && _ai_row "" "" "" "" "$(dim "${line#"  $TAB"}")"; continue ;;
      *"$TAB"*) ;;
      *) continue ;;
    esac
    id=${line%%"$TAB"*}; line=${line#*"$TAB"}; label=${line%%"$TAB"*}; status=
    case $line in *"$TAB"*) status=${line#*"$TAB"}; status=${status//"$TAB"/ } ;; esac   # extra fields join into status
    _ai_plugin_ok_id "$id" || { seen=; continue; }
    [ -z "$hdr" ] && { _ai_row "" "" "" "$(printf '\033[1;38;5;173m%s\033[0m' "$title")" ""; hdr=1; }
    _ai_row "plugin:$n" "$id" "$n" "$label" "$status"; seen=1
  done <<<"$out"
  [ -n "$hdr" ] || { _ai_row "" "" "" "$(printf '\033[1;38;5;173m%s\033[0m' "${title:-$n}")" ""; _ai_row "" "" "" "" "$(dim 'no entries')"; }
}
# exec the plugin's session in the foreground, in this folder
_ai_plugin_launch() {
  local n=$1 id=$2 d; shift 2
  _ai_plugin_ready "$n" && _ai_plugin_ok_id "$id" || { echo "plugin '$n' is not installed or is off (ai plugin ls)" >&2; return 1; }
  d=$(_ai_plugins_dir)/$n
  _ai_trust_share "$PWD"
  export AI_PLUGIN_DIR=$d
  if is_windows; then exec bash "$d/plugin" launch "$id" "$@"; else exec "$d/plugin" launch "$id" "$@"; fi
}

# ── ai plugin ls|add|update|setup|rm|on|off|do ──
_ai_plugin() {
  local sub=${1:-ls}; [ $# -gt 0 ] && shift
  case $sub in
    ls|add|new|update|setup|rm|on|off|do) "_ai_plugin_$sub" "$@" ;;
    *) echo "usage: ai plugin ls | add <folder|git url|owner/repo|cliproxy> [name] | new <name> | update [name] | setup|rm|on|off <name> | do <name> <action>"; return 1 ;;
  esac
}
# name must be valid; with "x" it must also be installed (a link counts)
_ai_plugin_arg() {
  _ai_plugin_ok_name "$1" || { echo "plugin name must be letters, digits, _ or -" >&2; return 1; }
  [ -z "${2:-}" ] || [ -e "$(_ai_plugins_dir)/$1" ] || [ -L "$(_ai_plugins_dir)/$1" ] || { echo "no plugin '$1' (ai plugin ls)" >&2; return 1; }
}
_ai_plugin_ls() {
  local n d st src title av
  for n in $(_ai_plugin_names); do
    d=$(_ai_plugins_dir)/$n; src=local; title=
    [ -s "$d/.source" ] && src=$(head -1 "$d/.source" | sed 's#^\([A-Za-z0-9+.-]*://\)[^/@]*@#\1#')
    if [ -e "$d/.off" ]; then st=off
    else st=on; title=$(_ai_plugin_call "$n" 2 rows | head -1 | tr -d '\000-\010\013-\037\177'); title=${title#section"$TAB"}; fi
    printf '\033[1;36m%s\033[0m %s %s \033[2m%s\033[0m\n' "$(pad "$n" 12)" "$(pad "$st" 3)" "$(pad "$title" 28)" "$src"
  done
  av=$(_ai_plugin_available)
  echo; echo "available"
  if [ -z "$av" ]; then echo "  no other bundled plugins"; return 0; fi
  while IFS="$TAB" read -r n title st; do
    printf '  \033[1;36m%s\033[0m %s%s\n' "$(pad "$n" 12)" "$title" "$([ "$st" = setup ] && echo ' (has setup)')"
  done <<<"$av"
}
# first line of <dir>/about, control chars stripped, 60 chars max
_ai_plugin_about() {
  local a; [ -f "$1/about" ] || return 0
  a=$(head -1 "$1/about" | tr -d '\000-\010\013-\037\177'); printf '%s\n' "${a:0:60}"
}
# bundled plugins not installed: name TAB about TAB setup|-
_ai_plugin_available() {
  local d n s
  for d in "${AI_HOME:-}"/plugins-extra/*/; do
    d=${d%/}; n=${d##*/}
    [ -f "$d/plugin" ] && _ai_plugin_ok_name "$n" || continue
    [ -e "$(_ai_plugins_dir)/$n" ] || [ -L "$(_ai_plugins_dir)/$n" ] && continue
    s=-; [ -f "$d/setup" ] && s=setup
    printf '%s\t%s\t%s\n' "$n" "$(_ai_plugin_about "$d")" "$s"
  done
}
# ai plugin new <name>: copy the template; setup is not run
_ai_plugin_new() {
  local n=${1:-} dir
  _ai_plugin_arg "$n" || return 1
  dir=$(_ai_plugins_dir)/$n
  { [ -e "$dir" ] || [ -L "$dir" ]; } && { echo "plugin '$n' exists"; return 1; }
  mkdir -p "${dir%/*}" && cp -R "${AI_HOME:-}/templates/plugin" "$dir" || return 1
  chmod +x "$dir/plugin" "$dir/setup"
  echo "created plugin $n in $dir: edit plugin, then run ai"
}
_ai_plugin_nouser() { sed 's#\(://\)[^/@ ]*@#\1#g'; }  # no user:token@ in output
_ai_plugin_add() {
  local src=${1:-} n=${2:-} dir url= from= p rc
  [ -n "$src" ] || { echo "usage: ai plugin add <folder|git url|owner/repo|cliproxy> [name]"; return 1; }
  while :; do case $src in ?*/) src=${src%/} ;; *) break ;; esac; done
  # same order as the TUI: bundled name, then local folder, then git
  if _ai_plugin_ok_name "$src" && [ -d "${AI_HOME:-}/plugins-extra/$src" ]; then from=$AI_HOME/plugins-extra/$src; [ -n "$n" ] || n=$src
  elif [ -d "$src" ]; then
    from=$src; p=${src##*/}
    case $p in ''|.|..) echo "give the plugin a folder name, not '$src'"; return 1 ;; esac
    [ -n "$n" ] || n=$p
  else
    case $src in
      git@*|ssh://*) echo "use an https:// URL"; return 1 ;;
      https://*|file://*) url=$src ;;  # file:// is for tests
      */*) case $src in */*/*|*/|/*|*[!A-Za-z0-9._/-]*) ;; *) url=https://github.com/$src.git ;; esac ;;
    esac
    [ -n "$url" ] || { echo "not a folder, git URL, owner/repo or bundled plugin: $src"; return 1; }
    has git || { echo "installing from git needs git"; return 1; }
    [ -n "$n" ] || { n=${url##*/}; n=${n%.git}; }
  fi
  _ai_plugin_arg "$n" || return 1
  dir=$(_ai_plugins_dir)/$n
  { [ -e "$dir" ] || [ -L "$dir" ]; } && { echo "plugin '$n' exists. remove it first: ai plugin rm $n"; return 1; }
  mkdir -p "${dir%/*}" || return 1
  _ai_plugin_add_in "$n" "$src" "$url" "$from"; rc=$?
  trap - INT TERM
  [ $rc = 0 ] || return $rc
  _ai_plugin_call "$n" 10 rows >/dev/null 2>&1   # macOS scans a new script on first run: pay for it now
  echo "installed plugin $n from $(echo "${url:-$from}" | _ai_plugin_nouser) in $dir"
}
# fetch into staging <plugins>/.<n>.install, check it, review and run its setup, then move it in.
# Any failure, a no, or Ctrl-C removes staging, so nothing half-installed is ever listed.
_ai_plugin_add_in() {
  local n=$1 src=$2 url=$3 from=$4 dir st err rc c
  dir=$(_ai_plugins_dir)/$n; st=${dir%/*}/.$n.install
  trap "rm -rf $(printf %q "$st"); exit 130" INT TERM
  rm -rf "$st"  # stale one from a crash; no trailing slash, so a link is removed, never followed
  if [ -n "$url" ]; then
    err=$(GIT_TERMINAL_PROMPT=0 git clone -q --depth 1 -- "$url" "$st" 2>&1 </dev/null) ||
      { rm -rf "$st"; echo "git clone failed: $(echo "$url" | _ai_plugin_nouser)"; echo "$err" | _ai_plugin_nouser; return 1; }
    printf '%s\n' "$url" > "$st/.source"
  else
    [ -f "$from/plugin" ] || { echo "no 'plugin' file in $from"; return 1; }
    mkdir "$st" && cp -R "$from"/. "$st"/ || { rm -rf "$st"; return 1; }
    rm -f "$st/.source"
  fi
  rm -f "$st/.off"  # new installs start on
  [ -f "$st/plugin" ] && { is_windows || [ -x "$st/plugin" ]; } ||
    { rm -rf "$st"; echo "no executable 'plugin' in $src"; return 1; }
  if [ -e "$st/setup" ] || [ -L "$st/setup" ]; then
    _ai_plugin_setup_ok "$st" && c=$(_ai_plugin_setup_copy "$st") || { rm -rf "$st"; return 1; }
    trap "rm -rf $(printf %q "$st") $(printf %q "$c"); echo 'setup failed (exit 130): plugin not installed'; exit 130" INT TERM
    _ai_plugin_approve "$c" || { rm -rf "$st" "$c"; echo "cancelled: plugin not installed"; return 1; }
    _ai_plugin_run_setup "$st" "$c"; rc=$?
    [ $rc = 0 ] || { rm -rf "$st"; echo "setup failed (exit $rc): plugin not installed"; return 1; }
  fi
  { [ -e "$dir" ] || [ -L "$dir" ]; } && { rm -rf "$st"; echo "plugin '$n' exists. remove it first: ai plugin rm $n"; return 1; }
  mv "$st" "$dir" || { rm -rf "$st"; return 1; }
}
# setup must be a regular file in the folder: a link could point anywhere and change after review
_ai_plugin_setup_ok() {
  [ ! -L "$1/setup" ] && { [ ! -e "$1/setup" ] || [ -f "$1/setup" ]; } || { echo "setup is a link: not run"; return 1; }
}
# show setup with line numbers; control bytes as ^X (like cat -v, but UTF-8 text stays readable)
_ai_plugin_review() {
  local i e=
  for i in 1 2 3 4 5 6 7 8 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 127; do
    e="${e}s/$(printf "\\$(printf %03o "$i")")/^$(printf "\\$(printf %03o $(( (i + 64) % 128 )))" | sed 's/[\\/&]/\\&/')/g;"
  done
  {
    echo "SETUP SCRIPT · review before running"
    echo "(the plugin's own code also runs with your permissions)"
    LC_ALL=C sed "$e" "$1" | awk '{ printf "%4d  %s\n", NR, $0 }'
  } | if [ -t 1 ] && has less; then less -R; else cat; fi
}
# copy <dir>/setup to a temp file and print its path: that copy is what gets shown and run
_ai_plugin_setup_copy() {
  local c
  c=$(mktemp "${TMPDIR:-/tmp}/ai-setup.XXXXXX") || return 1
  cp "$1/setup" "$c" || { rm -f "$c"; return 1; }
  printf '%s\n' "$c"
}
# _ai_plugin_approve <copy>: review, then ask (AI_YES=1 approves, for tests)
_ai_plugin_approve() {
  local ans
  _ai_plugin_review "$1"
  [ "${AI_YES:-}" = 1 ] && return 0
  read -r -p "run this setup script? [y/N] " ans
  case $ans in y|Y|yes) ;; *) return 1 ;; esac
}
# _ai_plugin_run_setup <dir> <copy>: run the reviewed copy with the user's terminal, no timeout,
# in the plugin folder, then remove the copy
_ai_plugin_run_setup() {
  local d=$1 c=$2 rc
  (cd "$d" && AI_PLUGIN_DIR=$d bash "$c"); rc=$?
  rm -f "$c"
  return $rc
}
# ai plugin setup <name>: run setup again in the installed folder; a failure keeps the plugin
_ai_plugin_setup() {
  local n=${1:-} d rc c
  _ai_plugin_arg "$n" x || return 1
  d=$(_ai_plugins_dir)/$n
  { [ -e "$d/setup" ] || [ -L "$d/setup" ]; } || { echo "plugin '$n' has no setup"; return 1; }
  _ai_plugin_setup_ok "$d" && c=$(_ai_plugin_setup_copy "$d") || return 1
  trap "rm -f $(printf %q "$c"); exit 130" INT TERM
  _ai_plugin_approve "$c" || { rm -f "$c"; trap - INT TERM; return 1; }
  [ -f "$d/config" ] && cp "$d/config" "$d/config.bak"
  _ai_plugin_run_setup "$d" "$c"; rc=$?
  trap - INT TERM
  [ $rc = 0 ] || echo "setup failed (exit $rc)"
  return $rc
}
_ai_plugin_update() {
  local n d rc=0 err old
  [ -z "${1:-}" ] || _ai_plugin_arg "$1" x || return 1
  for n in ${1:-$(_ai_plugin_names)}; do
    d=$(_ai_plugins_dir)/$n
    if [ -s "$d/.source" ] && [ -d "$d/.git" ]; then
      echo "updating $n"; old=$(cksum 2>/dev/null <"$d/setup")
      if err=$(GIT_TERMINAL_PROMPT=0 git -C "$d" pull -q --ff-only 2>&1 </dev/null); then
        [ "$old" = "$(cksum 2>/dev/null <"$d/setup")" ] || echo "setup changed: run setup again to apply"  # never run by itself
      else echo "$err" | _ai_plugin_nouser; rc=1; fi
    elif [ -n "${1:-}" ]; then echo "plugin '$n' was not installed from git"; return 1; fi
  done
  return $rc
}
_ai_plugin_rm() {
  local n=${1:-} ans d
  _ai_plugin_arg "$n" x || return 1
  d=$(_ai_plugins_dir)/$n
  if [ "${AI_YES:-}" != 1 ]; then
    read -r -p "delete plugin '$n' ($d) for good? [y/N] " ans
    case $ans in y|Y|yes) ;; *) return 1 ;; esac
  fi
  rm -rf "${d:?}" && echo "deleted plugin $n"  # no trailing slash: a link is removed, never followed
}
_ai_plugin_on() { _ai_plugin_arg "${1:-}" x && rm -f "$(_ai_plugins_dir)/$1/.off" && echo "plugin $1 on"; }
_ai_plugin_off() { _ai_plugin_arg "${1:-}" x && : > "$(_ai_plugins_dir)/$1/.off" && echo "plugin $1 off"; }
_ai_plugin_do() {
  local n=${1:-} a=${2:-} t=${AI_PLUGIN_DO_TIMEOUT:-30} rc
  _ai_plugin_arg "$n" x || return 1
  _ai_plugin_ok_id "$a" || { echo "usage: ai plugin do <name> <action>"; return 1; }
  _ai_plugin_ready "$n" || { echo "plugin '$n' is off (ai plugin on $n)"; return 1; }
  _ai_plugin_call "$n" "$t" do "$a"; rc=$?
  [ $rc = 137 ] && echo "plugin $n: stopped after ${t}s"
  return $rc
}
