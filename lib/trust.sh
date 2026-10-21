# shellcheck shell=bash
# Shared folder trust: when you launch an agent in a folder that any agent already trusts,
# mark it trusted in every agent, so none of them asks again. Needs jq; skipped without it.
# Only the exact folder counts (no parent folders), and a folder you marked untrusted
# in an agent is never overridden. Each agent needs two functions:
#   _ai_trust_<name>_has <dir>   exit 0 if trusted
#   _ai_trust_<name>_add <dir>   mark trusted (no-op if the agent has its own choice for it)
AI_TRUST="claude codex gemini qwen agy"

_ai_trust_share() {
  has jq || return 0
  # shortcut: off on Windows until each agent's Windows path format (C:/ vs C:\\) is verified there
  is_windows && return 0
  local d n hit=
  d=$(cd "$1" 2>/dev/null && pwd -P) || return 0  # agents store the resolved path (getcwd)
  case $d in *'"'*|*'\'*) return 0 ;; esac          # keep TOML/JSON quoting simple
  for n in $AI_TRUST; do "_ai_trust_${n}_has" "$d" && { hit=1; break; }; done
  [ -n "$hit" ] || return 0
  for n in $AI_TRUST; do "_ai_trust_${n}_has" "$d" || "_ai_trust_${n}_add" "$d"; done
}
# rewrite a JSON file with a jq filter, keeping its permissions; skips missing/invalid files
_ai_json_edit() {
  local f=$1 tmp; shift
  [ -r "$f" ] || return 0
  tmp=$(mktemp) || return 0
  if jq "$@" "$f" > "$tmp" 2>/dev/null && [ -s "$tmp" ]; then cat "$tmp" > "$f"; fi
  rm -f "$tmp"
}

# Claude: main config plus every account
_ai_trust_claude_files() {
  local f
  for f in "$HOME/.claude.json" "${CLA_ROOT:-$HOME/.claude-accounts}"/*/.claude.json; do [ -e "$f" ] && echo "$f"; done
}
_ai_trust_claude_has() {
  local f
  while IFS= read -r f; do
    jq -e --arg d "$1" '.projects[$d].hasTrustDialogAccepted == true' "$f" >/dev/null 2>&1 && return 0
  done <<EOF
$(_ai_trust_claude_files)
EOF
  return 1
}
_ai_trust_claude_add() {
  local f
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    jq -e --arg d "$1" '.projects[$d].hasTrustDialogAccepted == true' "$f" >/dev/null 2>&1 ||
      _ai_json_edit "$f" --arg d "$1" '.projects[$d].hasTrustDialogAccepted = true'
  done <<EOF
$(_ai_trust_claude_files)
EOF
}

# Codex: [projects."<dir>"] trust_level = "trusted" in config.toml
_ai_trust_codex_file() { echo "${CODEX_HOME:-$HOME/.codex}/config.toml"; }
_ai_trust_codex_level() {
  awk -v h="[projects.\"$1\"]" '$0 == h {s = 1; next} /^\[/ {s = 0}
    s && $1 == "trust_level" {gsub(/[" ]/, "", $3); print $3; exit}' "$(_ai_trust_codex_file)" 2>/dev/null
}
_ai_trust_codex_has() { [ "$(_ai_trust_codex_level "$1")" = trusted ]; }
_ai_trust_codex_add() {
  local f; f=$(_ai_trust_codex_file)
  [ -e "$f" ] || return 0
  [ -n "$(_ai_trust_codex_level "$1")" ] && return 0  # has a level (e.g. untrusted): leave it
  grep -qF "[projects.\"$1\"]" "$f" && return 0        # section exists without a level: leave it
  printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$1" >> "$f"
}

# Gemini / Qwen: trustedFolders.json {"<dir>": "TRUST_FOLDER" | "TRUST_PARENT" | "DO_NOT_TRUST"}
_ai_trust_gemini_has() { _ai_trust_gem_has "${GEMINI_CLI_HOME:-$HOME}/.gemini/trustedFolders.json" "$1"; }
_ai_trust_qwen_has()   { _ai_trust_gem_has "$HOME/.qwen/trustedFolders.json" "$1"; }
_ai_trust_gemini_add() { _ai_trust_gem_add "${GEMINI_CLI_HOME:-$HOME}/.gemini/trustedFolders.json" "$1"; }
_ai_trust_qwen_add()   { _ai_trust_gem_add "$HOME/.qwen/trustedFolders.json" "$1"; }
# TRUST_PARENT on a child also means <dir> is trusted
_ai_trust_gem_has() {
  jq -e --arg d "$2" '.[$d] == "TRUST_FOLDER" or
    any(to_entries[]; .value == "TRUST_PARENT" and (.key | sub("/[^/]*$"; "")) == $d)' "$1" >/dev/null 2>&1
}
_ai_trust_gem_add() {
  jq -e --arg d "$2" 'has($d)' "$1" >/dev/null 2>&1 && return 0  # any existing choice wins
  _ai_json_edit "$1" --arg d "$2" '.[$d] = "TRUST_FOLDER"'
}

# Antigravity (agy): trustedWorkspaces list in settings.json
_ai_trust_agy_file() { echo "$HOME/.gemini/antigravity-cli/settings.json"; }
_ai_trust_agy_has() {
  jq -e --arg d "$1" '(.trustedWorkspaces // []) | index($d) != null' "$(_ai_trust_agy_file)" >/dev/null 2>&1
}
_ai_trust_agy_add() {
  _ai_json_edit "$(_ai_trust_agy_file)" --arg d "$1" '.trustedWorkspaces = ((.trustedWorkspaces // []) + [$d])'
}
