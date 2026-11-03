# shellcheck shell=bash
# Shared by the three PATH shims, including the copies embedded in the standalone TUI.
_ai_wrap_warn() { printf 'warning: wrap %s\n' "$*" >&2; }
_ai_wrap_envname() {
  case $1 in ''|[0-9]*|*[!A-Za-z0-9_]*) return 1 ;; esac
  local flags; flags=$(declare -p "$1" 2>/dev/null); flags=${flags#declare -}; flags=${flags%% *}
  case $flags in *r*) return 1 ;; esac
}
_ai_wrap_terminal() { [ -t 0 ] && [ -t 2 ]; }
_ai_wrap_json() {
  local filter='if length == 1 and (.[0] | type == "object" and ((.env | type) == "null" or (.env | type) == "object")) then .[0] else error("invalid settings object/env") end'
  if [ -f "$1" ]; then jq -se "$filter" "$1"
  else printf '%s\n' "$1" | jq -se "$filter"; fi
}
_ai_wrap_combine() {
  jq -s '
    def merge($a; $b): reduce ($b | keys_unsorted[]) as $k ($a;
      .[$k] = (if (.[$k]|type)=="object" and ($b[$k]|type)=="object" then merge(.[$k]; $b[$k])
        elif (.[$k]|type)=="array" and ($b[$k]|type)=="array" then .[$k] + $b[$k]
        else $b[$k] end));
    merge(.[0]; .[1])' "$1" "$2"
}
# Keep the chosen owner and the pair: a new competing plugin must ask again.
_ai_wrap_owner() {
  local key=$1 previous=$2 next=$3 saved answer= chosen=$3 file
  file=${AI_PROVIDERS%/*}/plugins.owners
  saved=$(awk -F '\t' -v k="$key" -v a="$previous" -v b="$next" \
    '$1==k && $3==a && $4==b {v=$2} END {print v}' "$file" 2>/dev/null)
  case $saved in "$previous"|"$next") [ "$saved" = "$next" ]; return ;; esac
  if [ "${AI_YES:-}" != 1 ] && _ai_wrap_terminal; then
    printf "%s is set by %s and by %s. Use %s's? [Y/n] " "$key" "$previous" "$next" "$next" >&2
    IFS= read -r answer
    case $answer in n|N|no|NO) chosen=$previous ;; esac
  else
    _ai_wrap_warn "$key: $next replaces $previous (no prompt)"
  fi
  if mkdir -p "${file%/*}"; then
    (umask 077; printf '%s\t%s\t%s\t%s\n' "$key" "$chosen" "$previous" "$next" >> "$file") ||
      _ai_wrap_warn 'could not save owner choice'
  fi
  [ "$chosen" = "$next" ]
}
# $work, $merged and owner_* belong to _ai_wrap_main's dynamic scope.
_ai_wrap_merge() {
  local source=$1 plugin=$2 key previous fragment=$work/fragment.json
  cp "$source" "$fragment" || return 1
  for key in statusLine outputStyle theme; do
    jq -e --arg k "$key" 'has($k)' "$fragment" >/dev/null || continue
    eval 'previous=${owner_'"$key"':-}'
    if [ -n "$previous" ] && [ "$previous" != "$plugin" ] && ! _ai_wrap_owner "$key" "$previous" "$plugin"; then
      jq --arg k "$key" 'del(.[$k])' "$fragment" > "$work/next.json" && mv "$work/next.json" "$fragment" || return 1
    else
      eval 'owner_'"$key"'=$plugin'
      jq --arg k "$key" 'del(.[$k])' "$merged" > "$work/next.json" && mv "$work/next.json" "$merged" || return 1
    fi
  done
  _ai_wrap_combine "$merged" "$fragment" > "$work/next.json" && mv "$work/next.json" "$merged"
}
_ai_wrap_with_env() (
  while [ "$1" != -- ]; do
    case $1 in env) export "$2" ;; unset) unset "$2" ;; esac
    shift 2
  done
  shift
  "$@"
)
# Apply session env only at the leaf, never to the launcher's plugin/config lookup machinery.
_ai_wrap_plugin_exec() {
  local dir=$1 run=(); shift
  if is_windows; then run=("$(type -P bash)" "$dir/plugin"); else run=("$dir/plugin"); fi
  _ai_wrap_with_env "${env_changes[@]+"${env_changes[@]}"}" env "AI_PLUGIN_DIR=$dir" \
    env "AI_WRAP_AGENT=$agent" env "AI_WRAP_ENTRY=$AI_WRAP_ENTRY" env "AI_WRAP_TEAM=$AI_WRAP_TEAM" \
    env "AI_WRAP_BASE_URL=$AI_WRAP_BASE_URL" -- "${run[@]+"${run[@]}"}" "$@"
}
_ai_wrap_base_url() {
  local agent=$1 merged=$2 jq_on=$3 var value i
  case $agent in claude) var=ANTHROPIC_BASE_URL ;; codex) var=OPENAI_BASE_URL ;; gemini) var=GOOGLE_GEMINI_BASE_URL ;; esac
  eval 'value=${'"$var"':-}'
  for ((i=0; i<${#env_changes[@]}; i+=2)); do
    case ${env_changes[i+1]} in
      "$var="*) value=${env_changes[i+1]#*=} ;;
      "$var") value= ;;
    esac
  done
  if [ "$agent" = claude ] && [ -n "$jq_on" ]; then
    value=$(jq -r --arg k "$var" --arg fallback "$value" '.env[$k] // $fallback' "$merged")
  fi
  printf '%s\n' "$value"
}
_ai_wrap_main() (
  local agent=$1 shim real clean= part rest=$PATH work merged jq_on= n output line kind value name bad key file i rc
  local owner_statusLine= owner_outputStyle= owner_theme= saved_statusLine saved_outputStyle saved_theme changed= settings_count=0
  local args=() original=() sources=() fragments=() envlines=() additions=() env_changes=() hook_args=()
  shift; original=("$@")
  shim=$(cd -P "$(dirname "${BASH_SOURCE[0]}")/shim" && pwd) || return 1
  # Resolve aliases of the shim directory too, so a second PATH spelling cannot recurse.
  while :; do
    part=${rest%%:*}
    if [ "$(cd -P "${part:-.}" 2>/dev/null && pwd)" != "$shim" ]; then
      clean=$clean$part:
    fi
    case $rest in *:*) rest=${rest#*:} ;; *) break ;; esac
  done
  clean=${clean%:}
  real=$(PATH=$clean type -P "$agent")
  [ -n "$real" ] || { _ai_wrap_warn "$agent: real binary not found on PATH"; return 127; }
  # Hooks and the agent run without the shim, so a nested `claude` is never wrapped twice.
  PATH=$clean; export PATH
  work=$(mktemp -d "${TMPDIR:-/tmp}/ai-wrap.XXXXXX") || { "$real" "$@"; return; }
  trap 'rm -rf "$work"' EXIT
  merged=$work/settings.json
  printf '{}\n' > "$merged"
  if [ "$agent" = claude ] && has jq; then jq_on=1; fi
  # Claude accepts both a path and inline JSON (team mode uses inline JSON).
  while [ $# -gt 0 ]; do
    if [ -n "$jq_on" ]; then
      case $1 in
        --) args=("${args[@]+"${args[@]}"}" "$@"); break ;;
        --settings)
          if [ $# -gt 1 ]; then sources[${#sources[@]}]=$2; shift 2; continue; fi ;;
        --settings=*) sources[${#sources[@]}]=${1#*=}; shift; continue ;;
      esac
    fi
    args[${#args[@]}]=$1; shift
  done
  for value in "${sources[@]+"${sources[@]}"}"; do
    if _ai_wrap_json "$value" > "$work/source.json" 2>/dev/null &&
       _ai_wrap_combine "$merged" "$work/source.json" > "$work/next.json"; then
      mv "$work/next.json" "$merged"; changed=1
    else
      _ai_wrap_warn 'invalid existing --settings; leaving original arguments unchanged'
      args=("${original[@]+"${original[@]}"}"); printf '{}\n' > "$merged"; changed=; break
    fi
  done
  case ${AI_WRAP_ENTRY:-} in
    plugin:*/*)
      name=${AI_WRAP_ENTRY#plugin:}; name=${name%%/*}
      for key in statusLine outputStyle theme; do
        if [ -n "$jq_on" ] && jq -e --arg k "$key" 'has($k)' "$merged" >/dev/null; then
          eval 'owner_'"$key"'=$name'
        fi
      done ;;
  esac
  export AI_WRAP_AGENT=$agent AI_WRAP_TEAM=${AI_WRAP_TEAM:-0} AI_WRAP_ENTRY=${AI_WRAP_ENTRY:-}
  for n in $(_ai_plugin_ordered); do
    _ai_plugin_wraps "$n" || continue
    [ "$AI_WRAP_TEAM" != 1 ] || [ "$AI_PM_TEAM" = 1 ] || continue
    AI_WRAP_BASE_URL=$(_ai_wrap_base_url "$agent" "$merged" "$jq_on"); export AI_WRAP_BASE_URL
    hook_args=("${args[@]+"${args[@]}"}")
    [ -z "$changed" ] || hook_args=(--settings "$(native_path "$merged")" "${hook_args[@]+"${hook_args[@]}"}")
    output=$(_ai_plugin_call "$n" 10 wrap "$agent" "${hook_args[@]+"${hook_args[@]}"}") ||
      { _ai_wrap_warn "$n: failed or timed out; skipped"; continue; }
    fragments=(); envlines=(); additions=(); bad=; settings_count=0
    # Validate the entire response before applying anything from this plugin.
    while IFS= read -r line || [ -n "$line" ]; do
      [ -n "$line" ] || continue
      case $line in *"$TAB"*) ;; *) bad=1; break ;; esac
      kind=${line%%"$TAB"*}; value=${line#*"$TAB"}
      case $kind in
        env)
          case $value in *"$TAB"*) ;; *) bad=1; break ;; esac
          name=${value%%"$TAB"*}; value=${value#*"$TAB"}
          _ai_wrap_envname "$name" || { bad=1; break; }
          envlines[${#envlines[@]}]="env$TAB$name$TAB$value" ;;
        unset)
          _ai_wrap_envname "$value" || { bad=1; break; }
          envlines[${#envlines[@]}]="unset$TAB$value" ;;
        arg) additions[${#additions[@]}]=$value ;;
        settings)
          [ "$agent" = claude ] || continue
          if [ -z "$jq_on" ]; then settings_count=1; continue; fi
          file=$work/source.${#fragments[@]}.json
          [ -f "$value" ] && _ai_wrap_json "$value" > "$file" 2>/dev/null || { bad=1; break; }
          fragments[${#fragments[@]}]=$file ;;
        plugin-dir)
          [ "$agent" = claude ] || continue
          [ -n "$value" ] || { bad=1; break; }
          additions[${#additions[@]}]=--plugin-dir
          additions[${#additions[@]}]=$(native_path "$value") ;;
        *) bad=1; break ;;
      esac
    done <<< "$output"
    [ -z "$bad" ] || { _ai_wrap_warn "$n: bad output; skipped"; continue; }
    [ "$settings_count" = 0 ] || _ai_wrap_warn "$n: settings skipped (jq is missing)"
    if [ "${#fragments[@]}" -gt 0 ]; then
      cp "$merged" "$work/candidate.json" || { _ai_wrap_warn "$n: cannot stage settings; skipped"; continue; }
      saved_statusLine=$owner_statusLine saved_outputStyle=$owner_outputStyle saved_theme=$owner_theme
      merged=$work/candidate.json
      for file in "${fragments[@]+"${fragments[@]}"}"; do
        _ai_wrap_merge "$file" "$n" || { bad=1; break; }
      done
      merged=$work/settings.json
      if [ -n "$bad" ] || ! mv "$work/candidate.json" "$merged"; then
        owner_statusLine=$saved_statusLine owner_outputStyle=$saved_outputStyle owner_theme=$saved_theme
        _ai_wrap_warn "$n: settings merge failed; skipped"; continue
      fi
      changed=1
    fi
    args=("${args[@]+"${args[@]}"}" "${additions[@]+"${additions[@]}"}")
    for line in "${envlines[@]+"${envlines[@]}"}"; do
      kind=${line%%"$TAB"*}; value=${line#*"$TAB"}; name=${value%%"$TAB"*}
      if [ "$kind" = env ]; then
        value=${value#*"$TAB"}; env_changes[${#env_changes[@]}]=env; env_changes[${#env_changes[@]}]="$name=$value"
      else
        env_changes[${#env_changes[@]}]=unset; env_changes[${#env_changes[@]}]=$name
      fi
    done
    # Env instructions are the final settings overlay, including ones from earlier layers.
    if [ -n "$jq_on" ]; then
      for ((i=0; i<${#env_changes[@]}; i+=2)); do
        kind=${env_changes[i]}; value=${env_changes[i+1]}; name=${value%%=*}
        if [ "$kind" = env ]; then
          jq --arg k "$name" --arg v "${value#*=}" '.env[$k]=$v' "$merged" > "$work/next.json"
        else
          jq --arg k "$name" 'del(.env[$k])' "$merged" > "$work/next.json"
        fi
        mv "$work/next.json" "$merged"; changed=1
      done
    fi
  done
  if [ -n "$changed" ]; then
    chmod 600 "$merged"
    _ai_wrap_with_env "${env_changes[@]+"${env_changes[@]}"}" -- "$real" --settings "$(native_path "$merged")" "${args[@]+"${args[@]}"}"
  else
    _ai_wrap_with_env "${env_changes[@]+"${env_changes[@]}"}" -- "$real" "${args[@]+"${args[@]}"}"
  fi
  rc=$?
  return "$rc"
)
