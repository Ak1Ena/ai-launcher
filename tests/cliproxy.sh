#!/usr/bin/env bash
# Self-check for plugins-extra/cliproxy: fake HOME, fake proxy on 127.0.0.1 (python3), fake claude/brew.
# Never contacts a real proxy or the internet.   bash tests/cliproxy.sh   (also /bin/bash on macOS)
REPO=$(cd "$(dirname "$0")/.." && pwd)
T=$(cd "$(mktemp -d)" && pwd -P)
SRV=
trap '[ -n "$SRV" ] && { kill $SRV; wait $SRV; } 2>/dev/null; rm -rf "$T"' EXIT
export HOME=$T/home; mkdir -p "$HOME" "$T/bin" "$T/sys" "$T/jq" "$T/work"
unset CLIPROXY_API_KEY CLIPROXY_MGMT_KEY
export CLIPROXY_PROBE_SECS=5   # a loaded machine must not turn a slow answer into "proxy down"
PY=$(command -v python3 || true)
JQ=$(command -v jq || true)
# sys = /usr/bin + /bin without jq, brew, claude, so each test picks what is on PATH
for f in /usr/bin/* /bin/*; do
  b=${f##*/}; case $b in jq|brew|claude|python*) continue ;; esac
  [ -e "$T/sys/$b" ] || ln -s "$f" "$T/sys/$b"
done
[ -e "$T/sys/curl" ] || { c=$(command -v curl) && ln -s "$c" "$T/sys/curl"; }
[ -e "$T/sys/bash" ] || ln -s "$(command -v bash)" "$T/sys/bash"
[ -n "$JQ" ] && ln -s "$JQ" "$T/jq/jq"
BASEPATH=$T/bin:$T/jq:$T/sys
NOJQ=$T/bin:$T/sys
mkdir -p "$T/nocurl"; for f in "$T"/sys/*; do [ "${f##*/}" = curl ] || ln -s "$(readlink "$f")" "$T/nocurl/${f##*/}"; done
NOCURL=$T/bin:$T/jq:$T/nocurl

D=$T/plugins/cliproxy; mkdir -p "$T/plugins"; cp -R "$REPO/plugins-extra/cliproxy" "$D"
export AI_PLUGIN_DIR=$D
P=$D/plugin
CK=client-key-SECRET-123 MK=mgmt-key-SECRET-456
fails=0
ok() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }
has_line() { printf '%s\n' "$1" | grep -qF -- "$2"; }
run() { (cd "$T/work" && PATH=${RP:-$BASEPATH} "$P" "$@"); }   # plugin's cwd = user's folder
conf() { printf '%s\n' "$@" > "$D/config"; chmod 600 "$D/config"; }
wait_cache() { local i=0; while [ $i -lt 50 ]; do [ -f "$D/cache/usage" ] && [ "$(head -1 "$D/cache/usage")" != "${1:-}" ] && [ ! -d "$D/cache/lock" ] && return 0; sleep 0.2; i=$((i + 1)); done; return 1; }
TAB=$(printf '\t')

# fake claude: dumps env and args
printf '#!/bin/sh\nenv > "%s/claude.env"; printf "%%s\\n" "$@" > "%s/claude.args"\n' "$T" "$T" > "$T/bin/claude"; chmod +x "$T/bin/claude"

# ── no config, no key ──
rm -f "$D/config"
out=$(run rows)
ok "rows creates the config"           '[ -f "$D/config" ]'
ok "config is mode 600"                'case $(ls -l "$D/config") in -rw-------*) true;; *) false;; esac'
ok "first line is the section"         '[ "$(printf "%s\n" "$out" | head -1)" = "section${TAB}CLIPROXY · all accounts · failover" ]'
ok "no key hint names both options"    'has_line "$out" "all${TAB}Claude Code · all accounts${TAB}no key:" && has_line "$out" CLIPROXY_API_KEY && has_line "$out" "key="'

# ── proxy down ──
conf url=http://127.0.0.1:1 "key=$CK"
t0=$(date +%s); out=$(run rows); t1=$(date +%s)
ok "proxy down shows start hint"       'has_line "$out" "proxy not running — start: brew services start cliproxyapi"'
ok "proxy down answers fast"           '[ $((t1 - t0)) -lt 3 ]'
rm -f "$T/claude.env"
out=$(run launch all 2>&1); rc=$?
ok "launch with proxy down exits 1"    '[ $rc = 1 ] && has_line "$out" "proxy not running" && [ ! -e "$T/claude.env" ]'

# ── actions ──
ss() { run actions | grep -E "^(start|stop)${TAB}"; }
ok "no brew, no start_cmd: no actions" '[ -z "$(ss)" ]'
mkdir -p "$T/brewpfx/opt/cliproxyapi"
printf '#!/bin/sh\ncase "$1" in --prefix) echo "%s/brewpfx";; services) echo "brew $*" >> "%s/brew.log";; esac\n' "$T" "$T" > "$T/bin/brew"; chmod +x "$T/bin/brew"
out=$(ss)
ok "brew with cliproxyapi: start/stop" '[ "$out" = "start${TAB}start proxy
stop${TAB}stop proxy" ]'
run do stop >/dev/null 2>&1
ok "do stop runs brew services stop"   'grep -qx "brew services stop cliproxyapi" "$T/brew.log"'
rm -rf "$T/brewpfx/opt/cliproxyapi"
ok "brew without cliproxyapi: none"    '[ -z "$(ss)" ]'
rm -f "$T/bin/brew"
conf url=http://127.0.0.1:1 "start_cmd=touch $T/started"
ok "start_cmd only: start action"      '[ "$(ss)" = "start${TAB}start proxy" ]'
run do start >/dev/null 2>&1
ok "do start runs start_cmd"           '[ -e "$T/started" ]'
ok "unknown action fails"              '! run do nope >/dev/null 2>&1'

# ── launch, keys ──
if [ -z "$PY" ]; then echo "skip fake-proxy tests: no python3"; [ $fails = 0 ] && echo "all passed" && exit 0; exit 1; fi
cat > "$T/srv.py" <<'EOF'
import json, os, sys, time, datetime
from http.server import BaseHTTPRequestHandler, HTTPServer
T = sys.argv[1]; CK = sys.argv[2]; MK = sys.argv[3]
def iso(s): return (datetime.datetime.utcnow() + datetime.timedelta(seconds=s)).strftime('%Y-%m-%dT%H:%M:%S.123456+00:00')
def mode():
    try: return open(T + '/mode').read().strip()
    except Exception: return ''
CREDS = {"observed_at": "2026-10-26T10:00:00Z", "files": [
  {"auth_index": "c1", "provider": "claude", "status": "active", "disabled": False, "success": 40, "failed": 1},
  {"auth_index": "c2", "provider": "Claude", "status": "active", "disabled": False, "success": 2, "failed": 0},
  {"auth_index": "c3", "provider": "claude", "status": "disabled", "disabled": True, "success": 1, "failed": 2},
  {"auth_index": "x1", "provider": "codex", "status": "active", "disabled": False, "success": 7, "failed": 0},
  {"auth_index": "g1", "provider": "gemini", "status": "active", "disabled": False, "success": 5, "failed": 0}]}
def jfile(n):
    try: return json.load(open(T + '/' + n))
    except Exception: return None
def quota(ai):
    now = int(time.time())
    o = (jfile('quota.json') or {}).get(ai)
    if o is not None: return 200, o
    if ai == "c1": return 200, {"five_hour": {"utilization": 1.0, "resets_at": iso(3600 + 600)}, "seven_day": {"utilization": 50.0, "resets_at": iso(4 * 86400 + 600)}}
    if ai == "c2": return 200, {"five_hour": {"utilization": 49.0, "resets_at": iso(3 * 3600)}, "seven_day": {"utilization": 40, "resets_at": None}}
    if ai == "x1": return 200, {"plan_type": "plus", "rate_limit": {"allowed": True, "limit_reached": False,
        "primary_window": {"used_percent": 12, "limit_window_seconds": 18000, "reset_after_seconds": 7200 + 600},
        "secondary_window": {"used_percent": 30, "limit_window_seconds": 604800, "reset_at": now + 2 * 86400 + 600}}}
    return 404, {"error": "nope"}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, obj):
        b = json.dumps(obj).encode(); self.send_response(code); self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(b))); self.end_headers(); self.wfile.write(b)
    def auth(self, k): return self.headers.get('Authorization', '') == 'Bearer ' + k
    def handle_any(self, body):
        with open(T + '/srv.log', 'a') as f: f.write(self.command + ' ' + self.path + '\n')
        m = mode(); p = self.path.split('?')[0]
        if p == '/v1/models':
            if not self.auth(CK): return self.send(401, {"error": "bad key"})
            if jfile('models.json'): return self.send(200, jfile('models.json'))
            return self.send(200, {"object": "list", "data": [{"id": "claude-sonnet-4-5", "object": "model", "owned_by": "anthropic"}, {"id": "gpt-5", "object": "model", "owned_by": "openai"}]})
        if '/management/' in p:
            if 'slow' in m and 'credentials' in p: time.sleep(3)
            if 'forbid' in m: return self.send(403, {"error": "remote management key not set"})
            if not self.auth(MK): return self.send(401, {"error": "bad mgmt key"})
            if 'v0' in m and p.startswith('/v8/'): return self.send(404, {"error": "not found"})
            if p == '/v0/management/routing/strategy':
                if self.command == 'GET': return self.send(200, {"strategy": (jfile('v0strategy.json') or "round-robin")})
                open(T + '/put.log', 'a').write(self.command + ' ' + body + '\n'); return self.send(200, {"status": "ok"})
            if p == '/v8/management/config/routing' and self.command == 'GET':
                try: return self.send(200, json.load(open(T + '/routing.json')))
                except Exception: return self.send(404, {"error": "not found"})
            if p == '/v8/management/config' and self.command == 'PATCH':
                open(T + '/patch.log', 'a').write(body + '\n')
                r = json.load(open(T + '/routing.json')) if os.path.exists(T + '/routing.json') else {}
                r.update(json.loads(body).get('routing', {})); json.dump(r, open(T + '/routing.json', 'w'))
                return self.send(200, {"status": "ok"})
            if p in ('/v8/management/credentials', '/v0/management/auth-files'): return self.send(200, jfile('creds.json') or CREDS)
            if p in ('/v8/management/requests/api-call', '/v0/management/api-call'):
                req = json.loads(body or '{}'); code, obj = quota(req.get('auth_index'))
                return self.send(200, {"status_code": code, "header": {}, "body": json.dumps(obj)})
        return self.send(404, {"error": "not found"})
    def do_GET(self): self.handle_any('')
    def do_PUT(self): self.handle_any(self.rfile.read(int(self.headers.get('Content-Length') or 0)).decode())
    def do_PATCH(self): self.handle_any(self.rfile.read(int(self.headers.get('Content-Length') or 0)).decode())
    def do_POST(self): self.handle_any(self.rfile.read(int(self.headers.get('Content-Length') or 0)).decode())
s = HTTPServer(('127.0.0.1', 0), H)
open(T + '/port.tmp', 'w').write(str(s.server_address[1])); os.rename(T + '/port.tmp', T + '/port')
s.serve_forever()
EOF
"$PY" "$T/srv.py" "$T" "$CK" "$MK" >/dev/null 2>&1 &
SRV=$!
i=0; while [ ! -f "$T/port" ] && [ $i -lt 150 ]; do sleep 0.1; i=$((i + 1)); done
URL=http://127.0.0.1:$(cat "$T/port")

export MYKEY=env-key-SECRET-789
conf "url=$URL" key_env=MYKEY
export CLAUDE_CONFIG_DIR=$HOME/acc/one ANTHROPIC_API_KEY=sk-ant-SHOULD-GO
run launch all --foo "a b" >/dev/null 2>&1; rc=$?
ok "launch execs claude"               '[ $rc = 0 ] && [ -f "$T/claude.env" ]'
ok "launch passes args"                '[ "$(cat "$T/claude.args")" = "--foo
a b" ]'
ok "launch sets base url"              'grep -qx "ANTHROPIC_BASE_URL=$URL" "$T/claude.env"'
ok "launch sets gateway discovery"     'grep -qx "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1" "$T/claude.env"'
ok "launch unsets CLAUDE_CONFIG_DIR"   '! grep -q "^CLAUDE_CONFIG_DIR=" "$T/claude.env"'
ok "launch unsets ANTHROPIC_API_KEY"   '! grep -q "^ANTHROPIC_API_KEY=" "$T/claude.env"'
conf "url=$URL" key_env=MYKEY mgmt_key_env=MYMK
CLIPROXY_MGMT_KEY=$MK MYMK=$MK run launch all >/dev/null 2>&1
ok "launch drops the management keys"  '! grep -qe "^CLIPROXY_MGMT_KEY=" -e "^MYMK=" "$T/claude.env" && grep -qx "ANTHROPIC_AUTH_TOKEN=$MYKEY" "$T/claude.env"'
ok "key_env gives the token"           'grep -qx "ANTHROPIC_AUTH_TOKEN=$MYKEY" "$T/claude.env"'
conf "url=$URL/" key_env=MYKEY "key=$CK"
run launch all >/dev/null 2>&1
ok "key= wins over key_env="           'grep -qx "ANTHROPIC_AUTH_TOKEN=$CK" "$T/claude.env"'
ok "trailing slash dropped from url"   'grep -qx "ANTHROPIC_BASE_URL=$URL" "$T/claude.env"'
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY
printf 'url=%s\r\nkey=%s\r\n' "$URL" "$CK" > "$D/config"
run launch all >/dev/null 2>&1
ok "CRLF config parses"                'grep -qx "ANTHROPIC_BASE_URL=$URL" "$T/claude.env" && grep -qx "ANTHROPIC_AUTH_TOKEN=$CK" "$T/claude.env"'
conf "url=$URL" key_env=MYKEY "key=$CK"
ok "launch of another id exits 1"      '! run launch other >/dev/null 2>&1'

# ── up, no management key ──
out=$(run rows)
ok "proxy up status"                   'has_line "$out" "all${TAB}Claude Code · all accounts${TAB}proxy up"'
ok "no mgmt key: usage hint"           'has_line "$out" "  ${TAB}usage: set CLIPROXY_MGMT_KEY or mgmt_key= in config"'
out=$(RP=$NOJQ run rows)
ok "no jq: up, counts hidden, hint"    'has_line "$out" "proxy up" && has_line "$out" "usage needs jq" && ! has_line "$out" " req"'
out=$(RP=$NOCURL run rows)
ok "no curl: /dev/tcp probe says up"   'has_line "$out" "proxy up" && has_line "$out" "usage needs curl"'
ok "no curl: launch still works"       'RP=$NOCURL run launch all >/dev/null 2>&1'

# ── merged usage ──
conf "url=$URL" "key=$CK" "mgmt_key=$MK"
: > "$T/srv.log"
t0=$(date +%s); out=$(run rows); t1=$(date +%s)
ok "first rows says loading, fast"     'has_line "$out" "  ${TAB}loading usage…" && [ $((t1 - t0)) -lt 3 ]'
ok "refresh writes the cache"          'wait_cache'
out=$(run rows)
cl=$(printf '%s\n' "$out" | grep "^  ${TAB}claude ")
cx=$(printf '%s\n' "$out" | grep "^  ${TAB}codex ")
gm=$(printf '%s\n' "$out" | grep "^  ${TAB}gemini ")
ok "claude: avg 5h (1% is 1%, not a fraction)"    'case $cl in *"5h "*" 25% ↻1h"*) true;; *) false;; esac'
ok "claude: avg weekly, soonest reset" 'case $cl in *"wk "*" 45% ↻4d"*) true;; *) false;; esac'
ok "claude: accounts and requests"     'case $cl in *"3 accts · 43 req · 3 failed") true;; *) false;; esac'
ok "codex: windows by seconds"         'case $cx in *"5h "*" 12% ↻2h"*"wk "*" 30% ↻2d"*"1 acct · 7 req · 0 failed") true;; *) false;; esac'
ok "gemini: counts only"               '[ "$gm" = "  ${TAB}gemini · 1 acct · 5 req · 0 failed" ]'
ok "model names line"                  'has_line "$out" "  ${TAB}models: claude-sonnet-4-5, gpt-5"'
ok "detail lines follow the entry"     '[ "$(printf "%s\n" "$out" | sed -n 2p | cut -f1)" = all ] && [ -z "$(printf "%s\n" "$out" | sed -n "3,\$p" | grep -v "^  ${TAB}")" ]'
ok "never reads usage/queue"           '! grep -q "usage" "$T/srv.log"'
ok "v8 credentials used"               'grep -q "GET /v8/management/credentials" "$T/srv.log"'

# ── stale temp files are swept ──
touch -t 200001010000 "$D/cache/creds.99999"; touch "$D/cache/quota.99998"
rm -f "$D/cache/usage"; run rows >/dev/null; wait_cache
ok "old temp files swept, fresh kept"  '[ ! -e "$D/cache/creds.99999" ] && [ -e "$D/cache/quota.99998" ]'
rm -f "$D/cache/quota.99998"

# ── cache TTL ──
printf '%s\n  \tcached-marker\n' "$(date +%s)" > "$D/cache/usage"
: > "$T/srv.log"
out=$(run rows); sleep 1
ok "fresh cache is used as is"         'has_line "$out" cached-marker && ! grep -q management "$T/srv.log"'
old=$(( $(date +%s) - 61 ))
printf '%s\n  \tcached-marker\n' "$old" > "$D/cache/usage"
echo slow > "$T/mode"
t0=$(date +%s); out=$(run rows); t1=$(date +%s)
ok "stale cache answers < 2 s"         'has_line "$out" cached-marker && [ $((t1 - t0)) -lt 3 ]'
ok "stale cache is refreshed"          'wait_cache "$old" && grep -q "^  ${TAB}claude " "$D/cache/usage"'

# ── v0 fallback, refused management API ──
echo v0 > "$T/mode"; rm -f "$D/cache/usage"; : > "$T/srv.log"
run rows >/dev/null; wait_cache
out=$(run rows)
ok "v0 fallback gives the same lines"  'has_line "$out" "1 acct · 7 req · 0 failed" && grep -q "/v0/management/auth-files" "$T/srv.log" && grep -q "POST /v0/management/api-call" "$T/srv.log"'
echo forbid > "$T/mode"; rm -f "$D/cache/usage"
run rows >/dev/null; wait_cache
out=$(run rows)
ok "refused management API: hint"      'has_line "$out" "  ${TAB}usage: management API refused (403)"'
: > "$T/mode"

# ── session affinity ──
AFF='session affinity off'
rm -f "$T/routing.json" "$T/patch.log"; rm -rf "$D/cache"
run rows >/dev/null; wait_cache
out=$(run rows)
ok "routing unknown (404): no line"    '! has_line "$out" "session affinity off" && ! has_line "$out" "routing:"'
ok "routing unknown, mgmt key: action" 'has_line "$(run actions)" "affinity${TAB}Keep each chat on one account"'
echo '{"strategy":"round-robin","session-affinity":false,"session-affinity-subagents":true,"session-affinity-ttl":"30m"}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "affinity off: warning right under entry" '[ "$(printf "%s\n" "$out" | sed -n 3p)" = "  ${TAB}⚠ session affinity off: a chat can move between accounts and re-send its whole context — TUI: a, or ai plugin do cliproxy affinity" ]'
ok "affinity off: action listed"       'has_line "$(run actions)" "affinity${TAB}Keep each chat on one account"'
echo '{"strategy":"round-robin","session-affinity":true,"session-affinity-subagents":false}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "subagents off: warning"            'has_line "$out" "⚠ session affinity off"'
echo '{"strategy":"fill-first","session-affinity":true,"session-affinity-subagents":true,"session-affinity-ttl":"1h"}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "affinity on: routing line"         '[ "$(printf "%s\n" "$out" | sed -n 3p)" = "  ${TAB}routing: fill-first · sticky per chat (1h)" ] && ! has_line "$out" "⚠"'
ok "affinity on: no action"            '! run actions | grep -q affinity'
echo '{"strategy":"round-robin","session-affinity":false,"session-affinity-ttl":"2h"}' > "$T/routing.json"
res=$(run do affinity)
ok "do affinity sends the PATCH body"  '[ "$(cat "$T/patch.log")" = "{\"routing\":{\"session-affinity\":true,\"session-affinity-subagents\":true}}" ]'
ok "do affinity prints one result line" '[ "$(printf "%s\n" "$res" | wc -l | tr -d " ")" = 1 ] && has_line "$res" "session affinity on"'
ok "do affinity invalidates the cache" '[ ! -e "$D/cache/usage" ]'
echo '{"session-affinity":false}' > "$T/routing.json"; rm -f "$T/patch.log"
run do affinity >/dev/null
ok "empty ttl is set to 1h"            'has_line "$(cat "$T/patch.log")" "\"session-affinity-ttl\":\"1h\""'
rm -f "$T/routing.json"; echo v0 > "$T/mode"
ok "do affinity on a v0 proxy: says so" 'run do affinity | grep -q "not available"'
: > "$T/mode"
conf "url=$URL" "key=$CK"
rm -rf "$D/cache"
out=$(run rows); res=$(run do affinity 2>&1)
ok "no mgmt key: no line, no action"   '! has_line "$out" "affinity" && ! has_line "$out" "routing:" && ! run actions | grep -q affinity'
ok "no mgmt key: do affinity refuses"  'has_line "$res" "management key"'
conf url=http://127.0.0.1:1 "mgmt_key=$MK"
ok "dead proxy: no answer, not HTTP 000" 'run do affinity | grep -q "not available (no answer;"'

# ── mode: failover (default) / load balance ──
conf "url=$URL" "key=$CK" "mgmt_key=$MK" "# a comment"
: > "$T/mode"; rm -f "$T/patch.log" "$T/put.log"
echo '{"strategy":"round-robin","session-affinity":true,"session-affinity-subagents":true}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
MIS="⚠ proxy uses round-robin, plugin mode is failover — apply: TUI a, or ai plugin do cliproxy apply"
ok "default mode: failover title"      '[ "$(printf "%s\n" "$out" | head -1)" = "section${TAB}CLIPROXY · all accounts · failover" ]'
ok "mismatch: warning line"            'has_line "$out" "  ${TAB}$MIS"'
ok "no ttl: plain sticky per chat"     'has_line "$out" "routing: round-robin · sticky per chat" && ! has_line "$out" "sticky per chat ("'
acts=$(run actions)
ok "actions: modes, apply on mismatch" 'has_line "$acts" "mode-failover${TAB}Mode: failover (one account until limited)" && has_line "$acts" "mode-balance${TAB}Mode: load balance" && has_line "$acts" "apply${TAB}Apply failover to the proxy"'
res=$(run do mode-failover); rc=$?
ok "mode-failover: one PATCH, exact body" '[ $rc = 0 ] && [ "$(cat "$T/patch.log")" = "{\"routing\":{\"strategy\":\"fill-first\",\"cooldown\":{\"disable-cooling\":false}}}" ]'
ok "mode-failover: says so"            'has_line "$res" "proxy now uses failover"'
ok "mode written, other lines kept, 600" '[ "$(cat "$D/config")" = "url=$URL
key=$CK
mgmt_key=$MK
# a comment
mode=failover" ] && case $(ls -l "$D/config") in -rw-------*) true;; *) false;; esac'
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "matching proxy: no mismatch, no apply" '! has_line "$out" "proxy uses" && ! run actions | grep -q "^apply"'
rm -f "$T/patch.log"; res=$(run do mode-balance)
ok "mode-balance: exact body"          '[ "$(cat "$T/patch.log")" = "{\"routing\":{\"strategy\":\"round-robin\",\"session-affinity\":true,\"session-affinity-subagents\":true,\"session-affinity-ttl\":\"1h\"}}" ]'
ok "mode-balance: one mode line"       '[ "$(grep -c "^mode=" "$D/config")" = 1 ] && grep -qx mode=balance "$D/config"'
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "balance title, no mismatch"        '[ "$(printf "%s\n" "$out" | head -1)" = "section${TAB}CLIPROXY · all accounts · load balance" ] && ! has_line "$out" "proxy uses"'
echo '{"strategy":"wrr","session-affinity":false,"session-affinity-ttl":"2h"}' > "$T/routing.json"; rm -f "$T/patch.log"
run do apply >/dev/null
ok "balance keeps weighted-round-robin" '[ "$(cat "$T/patch.log")" = "{\"routing\":{\"strategy\":\"weighted-round-robin\",\"session-affinity\":true,\"session-affinity-subagents\":true}}" ]'
conf "url=$URL" "key=$CK" "mgmt_key=$MK" mode=failover
echo '{"strategy":"fill-first","cooldown":{"disable-cooling":true}}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows)
ok "failover with cooling off: mismatch" 'has_line "$out" "⚠ proxy uses fill-first (cooling off), plugin mode is failover"'
echo v0 > "$T/mode"; rm -f "$T/patch.log"
run do mode-failover >/dev/null; r1=$?
echo '"wrr"' > "$T/v0strategy.json"; run do mode-balance >/dev/null
ok "v0: strategy PUT only"             '[ $r1 = 0 ] && [ ! -e "$T/patch.log" ] && [ "$(cat "$T/put.log")" = "PUT {\"value\":\"fill-first\"}
PUT {\"value\":\"weighted-round-robin\"}" ]'
: > "$T/mode"; rm -f "$T/put.log" "$T/v0strategy.json"
conf "url=$URL" "key=$CK"
res=$(run do mode-balance); rc=$?
ok "no mgmt key: config only"          '[ $rc = 0 ] && grep -qx mode=balance "$D/config" && has_line "$res" "proxy not changed (no management key)" && [ ! -e "$T/patch.log" ] && [ ! -e "$T/put.log" ]'
ok "no mgmt key: no apply action"      '! run actions | grep -q "^apply"'
ok "no mgmt key: do apply refuses"     '! run do apply >/dev/null'

# ── proxy text cannot inject control chars ──
conf "url=$URL" "key=$CK" "mgmt_key=$MK"
printf '%s' '{"data":[{"id":"evil\u001b[31m\nall\tFORGED"}]}' > "$T/models.json"
printf '%s' '{"files":[{"auth_index":"z1","provider":"ev\u001bil\nall\tFORGED2","success":1,"failed":0}]}' > "$T/creds.json"
echo '{"strategy":"x\u001b]0;t\u0007\nall\tF3","session-affinity":true,"session-affinity-ttl":"1\u001bh"}' > "$T/routing.json"
rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows; run actions)
ok "no ESC or forged rows from proxy text" '! printf "%s" "$out" | grep -q "$(printf "\033")" && [ "$(printf "%s\n" "$out" | grep -c "^all${TAB}")" = 1 ] && has_line "$out" "models: evil[31mallFORGED" && has_line "$out" "evilallforged2 ·"'
rm -f "$T/models.json" "$T/creds.json" "$T/routing.json"
conf "url=$URL" "key=$CK" "mgmt_key=$MK"

# ── all accounts limited ──
LIM='{"five_hour":{"utilization":100,"resets_at":"RESET"},"seven_day":{"utilization":10,"resets_at":null}}'
FREE='{"five_hour":{"utilization":5,"resets_at":null},"seven_day":{"utilization":5,"resets_at":null}}'
fresh() { rm -rf "$D/cache"; run rows >/dev/null; wait_cache; out=$(run rows); }
cred() { printf '{"auth_index":"%s","provider":"%s","status":"active","disabled":false,"success":1,"failed":0%s}' "$1" "$2" "${3:+,$3}"; }
rst() { python3 -c "import datetime,sys;print((datetime.datetime.utcnow()+datetime.timedelta(seconds=int(sys.argv[1]))).strftime('%Y-%m-%dT%H:%M:%S+00:00'))" "$1"; }
r1=$(rst 7500); r2=$(rst 3900)
echo "{\"c1\":${LIM/RESET/$r1},\"c2\":${LIM/RESET/$r2}}" > "$T/quota.json"
echo "{\"files\":[$(cred c1 claude),$(cred c2 claude),$(cred c3 claude '"disabled":true'),$(cred x1 codex)]}" | sed 's/"success":1,"failed":0,"disabled":true/"success":1,"failed":0/;s/"auth_index":"c3","provider":"claude","status":"active","disabled":false/"auth_index":"c3","provider":"claude","status":"active","disabled":true/' > "$T/creds.json"
fresh
ok "all claude limited: warning first, soonest reset" '[ "$(printf "%s\n" "$out" | sed -n 3p)" = "  ${TAB}⚠ all Claude accounts limited · next reset ↻1h" ]'
ok "all claude limited: status says so"   'has_line "$out" "all${TAB}Claude Code · all accounts${TAB}proxy up · Claude limited"'
ok "warning not repeated by usage"        '[ "$(printf "%s\n" "$out" | grep -c "⚠ all")" = 1 ]'
echo "{\"c1\":${LIM/RESET/$r1},\"c2\":$FREE}" > "$T/quota.json"
fresh
ok "one claude free: no warning"          '! has_line "$out" "⚠ all" && has_line "$out" "${TAB}proxy up" && ! has_line "$out" "Claude limited"'
echo "{\"c1\":$FREE,\"c2\":$FREE,\"x1\":{\"rate_limit\":{\"limit_reached\":true,\"primary_window\":{\"used_percent\":100,\"limit_window_seconds\":18000,\"reset_after_seconds\":7200}}}}" > "$T/quota.json"
fresh
ok "all codex limited: codex warning, status unchanged" 'has_line "$out" "⚠ all Codex accounts limited · next reset ↻2h" && ! has_line "$out" "Claude limited" && ! has_line "$out" "⚠ all Claude"'
echo "{\"files\":[$(cred x1 codex),$(cred g1 gemini)]}" > "$T/creds.json"; echo '{}' > "$T/quota.json"
fresh
ok "no claude creds: nothing about Claude" '! has_line "$out" "Claude accounts" && ! has_line "$out" "Claude limited"'
echo "{\"files\":[$(cred c1 claude '"cooldowns":[{"scope":"auth","reason":"quota","remaining_seconds":5400}]'),$(cred c2 claude '"unavailable":true')]}" > "$T/creds.json"; echo '{"c1":'"$FREE"',"c2":'"$FREE"'}' > "$T/quota.json"
fresh
ok "cooldown/unavailable alone counts"    'has_line "$out" "⚠ all Claude accounts limited · next reset ↻1h" && has_line "$out" "proxy up · Claude limited"'
rm -f "$T/creds.json" "$T/quota.json"; rm -rf "$D/cache"

# ── key safety ──
all=$(run rows; run actions; run launch nope 2>&1; run do apply; run do mode-failover; cat "$D"/cache/* "$T"/*.log 2>/dev/null)
ok "keys never printed or cached"      '! printf "%s" "$all" | grep -qe "$CK" -e "$MK" -e "$MYKEY"'
ok "cache is private"                  'case $(ls -ld "$D/cache") in drwx------*) true;; *) false;; esac'

[ $fails = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
