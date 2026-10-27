# CLIProxyAPI facts for the cliproxy plugin (Task 1)

Sources (shallow clones, pinned):
- CPA = router-for-me/CLIProxyAPI @ `307803510bce87f986fcf7b1a84190f105c7b4aa` (module path `.../CLIProxyAPI/v8`)
- MC = router-for-me/Cli-Proxy-API-Management-Center @ `714de0760a3ce86094b178fb46f834d34bc4a2a5`

Link form: `https://github.com/router-for-me/CLIProxyAPI/blob/307803510bce87f986fcf7b1a84190f105c7b4aa/<file>#L<line>`.
Below, `CPA:<file>:<lines>` / `MC:<file>:<lines>` use that base. Default port 8317 (CPA:config.example.yaml:30).

## Summary table

| Need | Answer |
| --- | --- |
| Client key header | `Authorization: Bearer <key>` (also `X-Api-Key`, `X-Goog-Api-Key`, `?key=`, `?auth_token=`) |
| Management key header | `Authorization: Bearer <mgmt-key>` or `X-Management-Key: <mgmt-key>` |
| Credentials list | `GET /v8/management/credentials` (v0: `GET /v0/management/auth-files`), same handler |
| Per-model requests/tokens | **NOT FOUND** as an aggregate endpoint (see section 4) |
| Quota used % + reset | Not from CPA directly; call upstream via `POST /v8/management/requests/api-call` (v0 `/v0/management/api-call`) |
| Gemini quota | **NOT FOUND** (MC has no Gemini-CLI quota provider; only antigravity, claude, codex, devin, kimi, meta, xai, plugin) |

## 1. Auth

### Management API
- Accepts `Authorization: Bearer <key>` (a header without the `Bearer ` prefix is taken as the raw key) or `X-Management-Key`. CPA:internal/api/handlers/management/handler.go:323-354 (extraction 333-345). Doc: CPA:docs/management-api-v8.md:8-13.
- Key comes from config `management.secret-key` (plaintext is hashed on startup) or env `MANAGEMENT_PASSWORD`. CPA:config.example.yaml:78-89 (`secret-key: ""` empty = Management API disabled, 404 on both `/v0/management` and `/v8/management`); CPA:internal/api/handlers/management/handler.go:75 (env), :426 (403 "remote management key not set" when neither).
- Non-localhost clients are refused unless `management.allow-remote: true`. CPA:config.example.yaml:84; handler.go:397-398. 5 failed attempts bans the IP for 30 min. handler.go:359-360.
- v8 and v0 use the same key and policy. CPA:docs/management-api-v8.md:10-11. Both are registered: v8 at CPA:internal/api/server_management_v8.go:12-67, v0 at CPA:internal/api/server_management.go:43-188.
- MC sends `Authorization: Bearer <managementKey>` to base `<url>/v8/management`. MC:src/services/api/client.ts:126-131; MC:src/utils/constants.ts:17; MC:src/utils/connection.ts:17.

### Client (inference) key: `/v1/models`
- Route `GET /v1/models` is behind `AuthMiddleware(accessManager)`. CPA:internal/api/server_routes.go:64-68.
- Key list is `access.api-keys` in v8 config (`/config/access/api-keys`, doc CPA:docs/management-api-v8.md:39) / top-level `api-keys` in v0 (:237-238); config.example.yaml:108.
- Accepted places, first match wins: `Authorization: Bearer`, `X-Goog-Api-Key`, `X-Api-Key`, `?key=`, `?auth_token=`. CPA:internal/access/config_access/provider.go:55-100.
- When no client keys are configured the provider returns "not handled" (provider.go:60-62), so the request is not rejected by it. Plugin should still send a key if it has one.
- Response shape depends on the caller: an `Anthropic-Version` header or a `User-Agent` starting `claude-cli` gets the Anthropic shape (CPA:server_routes.go:606-611, 640-643); everything else gets the OpenAI shape (:644-645). Plugin should use plain `curl` (no such header/UA) to get the OpenAI shape.
  - Anthropic shape also rewrites ids that do not start with `claude-` into `claude-fable-5-dd-<reversed id>` (CPA:internal/client/claude/models/models.go:12-19, 47-52), so do not use it to list model names.

OpenAI-shape example (CPA:sdk/api/handlers/openai/openai_handlers.go:81-107; fields `id`, `object`, optional `created`, `owned_by`):
```json
{"object":"list","data":[
  {"id":"claude-sonnet-4-5","object":"model","created":1759104000,"owned_by":"anthropic"},
  {"id":"gpt-5","object":"model","created":1754524800,"owned_by":"openai"}
]}
```
(ids/values illustrative; shape from source.)

## 2. Credentials: `GET /v8/management/credentials`

- Route: CPA:server_management_v8.go:43 -> `ListAuthFiles`. v0 equivalent `GET /v0/management/auth-files`: CPA:server_management.go:180. Same handler: CPA:internal/api/handlers/management/auth_files.go:106.
- Response: `{"observed_at": <RFC3339>, "files": [ ... ]}` (non-paginated; auth_files.go:176). Files sorted by lower-case name (:171-175). Optional filters `?name=`, `?auth_index=` (:121-122); `?page=&page_size=` switches to a paginated envelope built by `authFilesListResponse` (:133-156; envelope fields NOT READ, avoid pagination).
- Each entry (auth_files.go:668-760; line refs are `buildAuthFileEntry`):
  - always: `id`, `auth_index`, `name`, `type`, `provider` (both = provider, e.g. `claude`, `codex`, `antigravity`), `label`, `status`, `status_message`, `disabled`, `unavailable`, `runtime_only`, `source`, `size`, `success`, `failed`, `recent_requests`, `quota`, `cooldowns` (668-685, plus :165-166 / :179-181)
  - optional: `model_quotas`, `supports_quota`, `quota_provider`, `quota_plugin_id`, `quota_probe`, `email`, `project_id`, `account_type`, `account`, `created_at`, `modtime`, `updated_at`, `last_refresh`, `next_retry_after`, `path`, `id_token` (claims object), `priority` (686-759)
  - Codex only, additionally (CPA:docs/management-api-v8.md:214-222): `siwc_bind_supported`, `use_siwc`, `siwc_status` (`off|valid|invalid`), `siwc_bound`, `siwc_expires_at`
- `status` values: `unknown|pending|refreshing|active|error|disabled`. CPA:sdk/cliproxy/auth/status.go:8-18.
- `success` / `failed` are per-credential lifetime counters since process start (summed over all models); `recent_requests` is a list of `{time, success, failed}` buckets. CPA:sdk/cliproxy/auth/types.go:304-308, auth_files.go:682-684. There is no per-model counter or token count here.
- `cooldowns`: array of `{scope, model_key?, reason, retry_at, remaining_seconds, backoff_level?, http_status?}`, or `null` in home mode. CPA:sdk/cliproxy/auth/cooldown_view.go:11-19; auth_files.go:165-166.
- `quota` is a passive snapshot: `{"observed_at": <RFC3339, omitted if never>, "signals": {<Header-Name>: "<value>"}}`; only for providers `claude`, `codex`, `devin`, otherwise `signals` is `{}`. CPA:auth_files.go:813-834; CPA:sdk/cliproxy/auth/quota_signals.go:17-24. Signals are upstream response headers captured on the last request: `Retry-After`, `anthropic-ratelimit-unified-*` (claude), and for codex `x-codex-active-limit`, `x-codex-plan-type`, `x-codex-credits-*`, `x-codex-*-used-percent|-window-minutes|-reset-after-seconds|-reset-at|-allowed|-limit-reached|-limit-name`. quota_signals.go:170-215. Free (no upstream call) but stale until a request goes through that credential, and exact claude header names beyond the `anthropic-ratelimit-unified-` prefix are NOT FOUND in source (not enumerated).
- Credential `type` for grouping providers: use `.provider` (lower-cased by plugin).

Example `files[]` entry (shape from source; values illustrative):
```json
{"observed_at":"2026-10-26T10:00:00Z","files":[{
  "id":"claude-alice@example.com.json","auth_index":"a1b2c3d4e5f6a7b8","name":"claude-alice@example.com.json",
  "type":"claude","provider":"claude","label":"","status":"active","status_message":"",
  "disabled":false,"unavailable":false,"runtime_only":false,"source":"file","size":1234,
  "success":42,"failed":1,
  "recent_requests":[{"time":"10:00-10:10","success":3,"failed":0}],
  "quota":{"observed_at":"2026-10-26T09:58:00Z","signals":{"Anthropic-Ratelimit-Unified-5H-Utilization":"0.12"}},
  "cooldowns":[],
  "email":"alice@example.com","account_type":"oauth","account":"alice@example.com","priority":0
}]}
```
(`recent_requests[].time` format and the exact signal header names are illustrative; types are from source. Parse defensively.)

## 3. `POST /v8/management/requests/api-call` (v0: `POST /v0/management/api-call`)

- Routes: CPA:server_management_v8.go:31; CPA:server_management.go:73. Handler `APICall`, CPA:internal/api/handlers/management/api_tools.go:104. Timeout default 60 s (api_tools.go:24).
- Auth: management key (see section 1).
- Request (CPA:api_tools.go:33-42, doc :64-80):
```json
{"auth_index":"<from credentials list>","method":"GET","url":"https://api.anthropic.com/api/oauth/usage",
 "header":{"Authorization":"Bearer $TOKEN$","anthropic-beta":"oauth-2025-04-20","User-Agent":"claude-cli/2.1.280 (external, cli)"},
 "data":"", "proxy_url":""}
```
  `auth_index` also accepted as `authIndex` / `AuthIndex`. `method` and absolute `url` required (else 400). `$TOKEN$` in header values is replaced by the credential's `metadata.access_token`, else `attributes.api_key`, else `metadata.token/id_token/cookie`. `data` is a raw string body.
- Response, HTTP 200 whenever the call itself ran (api_tools.go:44-48, 88-91):
```json
{"status_code":200,"header":{"Content-Type":["application/json"]},"body":"<upstream body as a STRING>"}
```
  `body` is a string: the plugin must JSON-decode it a second time (`jq '.body|fromjson'`). MC does the same: MC:src/services/api/apiCall.ts:25-48, 79-96. Upstream errors show up in `status_code`, not as HTTP errors.

## 4. Usage / per-model breakdown

- v8 usage routes are only `GET /v8/management/observability/usage/api-keys` and `GET /v8/management/observability/usage/queue`. CPA:server_management_v8.go:40-41; doc CPA:docs/management-api-v8.md:99-100. v0: `/v0/management/api-key-usage`, `/v0/management/usage-queue`, plus the `usage-statistics-enabled` toggle. CPA:server_management.go:64-66, 92-93.
- `usage/api-keys`: only upstream `api_key` credentials (not OAuth logins), shape `{ "<provider>": { "<base_url>|<api_key>": {"success":N,"failed":N,"recent_requests":[{time,success,failed}]} } }`; no model, no tokens; and the map key contains the raw upstream API key (secret). CPA:internal/api/handlers/management/api_key_usage.go:12-16, 56-117. Not useful for this plugin.
- `usage/queue?count=N` (default 1): **destructive** pop of the oldest N events (`redisqueue.PopOldest`), returns a JSON array of records. CPA:internal/api/handlers/management/usage.go:23-55. Only filled when `usage-statistics-enabled` is true (CPA:internal/redisqueue/plugin.go:30-32; config.example.yaml:1181, default false). Events are retained for a limited time and consumed by whoever pops them (MC does not call it; no hits in MC:src). A plugin that pops would steal events from other consumers, so the plugin must not use it. A plugin could keep its own running totals only if it is the sole consumer; not recommended.
- Record shape if ever needed (CPA:internal/redisqueue/plugin.go:130-179 and the `queuedUsageDetail` struct below it):
```json
{"timestamp":"...","latency_ms":1200,"ttft_ms":300,"source":"...","auth_index":"a1b2...","tokens":{"input_tokens":10,"output_tokens":20,"reasoning_tokens":0,"cached_tokens":0,"cache_read_tokens":0,"cache_creation_tokens":0,"total_tokens":30},
 "failed":false,"provider":"claude","model":"claude-sonnet-4-5","alias":"...","endpoint":"...","auth_type":"oauth","api_key":"<client key!>","fail":{"status_code":200,"body":""}}
```
  Contains the caller's client `api_key`; treat as secret.
- **Per-model requests/tokens as a GET aggregate: NOT FOUND** in CPA (v8 or v0) and not used by MC (the MC quota/usage code only calls `/requests/api-call`, credentials, and plugin quota). Searched: `usage`, `statistics`, `/usage` routes in `internal/api/server_management*.go`; MC `src/` for `usage/queue|observability/usage|usage-queue` returns only `services/api/apiKeyUsage.ts` and config search index.
- Consequence for the plan's "one line per model (`<model> <n> req <tokens> tok`)": cannot be done from a stable read-only endpoint. Options: drop per-model lines; or show per-provider totals from credentials `success`/`failed` (requests only, no tokens, no model); or show the model list from `/v1/models` (names only).
- Per-model state does exist inside each credential (`model_quotas`, `cooldowns[].model_key`), but only as quota/cooldown observations, not request counts. CPA:auth_files.go:686-687, 836-851 (body of `modelQuotaObservationPayload` NOT READ in full).

## 5. Provider upstream quota (what MC calls via api-call)

MC calls `apiCallApi.request({authIndex, method, url, header})` -> `POST /requests/api-call` for each credential. MC:src/features/quota/providers/claude/data.ts:185-249, codex/data.ts:418-460.

### Claude (also needs `provider=="claude"`)
- URL `GET https://api.anthropic.com/api/oauth/usage` (MC:src/utils/quota/constants.ts:103). Also profile `GET https://api.anthropic.com/api/oauth/profile` (:101) for plan type.
- Headers (MC:constants.ts:105-110): `User-Agent: claude-cli/2.1.280 (external, cli)`, `Authorization: Bearer $TOKEN$`, `Content-Type: application/json`, `anthropic-beta: oauth-2025-04-20`; MC adds `Accept: application/json` for usage (claude/data.ts:197).
- Windows (MC:constants.ts:112-120; types MC:src/types/quota.ts:100-173): keys `five_hour` (rolling 5 h), `seven_day` (weekly), `seven_day_oauth_apps`, `seven_day_opus`, `seven_day_sonnet`, `seven_day_cowork`, `iguana_necktie` (weekly, "fable"); each `{utilization, resets_at}`. `utilization` = used %, `resets_at` = ISO-8601 or null. Period is derived from the key (claude/data.ts:103-106); comment says payload does not state period. Used % scale (0-100 vs 0-1) is not stated in MC source read: `normalizeNumberValue(window.utilization)` is shown as a percent (claude/data.ts:95-107). Treat as 0-100 and clamp.
- Also `limits[]` entries `{kind, group, percent, resets_at, is_active, scope.model.display_name}` (kind `weekly_scoped` for per-model weekly caps, claude/data.ts:50-63; types.ts:108-117) and `extra_usage`. Plugin only needs `five_hour` and `seven_day`.
```json
{"five_hour":{"utilization":23.0,"resets_at":"2026-10-26T15:00:00.000000+00:00"},
 "seven_day":{"utilization":41.0,"resets_at":"2026-10-30T09:00:00.000000+00:00"},
 "seven_day_opus":null,"seven_day_sonnet":{"utilization":10.0,"resets_at":"..."},"limits":[],"extra_usage":null}
```
(values illustrative; keys from MC types.) Wrapped in api-call as `body` string.

### Codex
- URL `GET https://chatgpt.com/backend-api/wham/usage` (MC:constants.ts:127). Headers (MC:constants.ts:134-138): `Authorization: Bearer $TOKEN$`, `Content-Type: application/json`, `User-Agent: codex-tui/0.149.1 (Mac OS 26.5.2; arm64) iTerm.app/3.6.11 (codex-tui; 0.149.1)`, plus `Chatgpt-Account-Id: <id>` when known (codex/data.ts:307-316; id from the credential's id_token claims, `resolveCodexChatgptAccountId`, body NOT READ).
- Payload (MC:src/types/quota.ts:35-97; snake_case or camelCase both accepted): `rate_limit: {allowed, limit_reached, primary_window, secondary_window}`; window `{used_percent, limit_window_seconds, reset_after_seconds, reset_at}`; also `code_review_rate_limit`, `additional_rate_limits[]`, `plan_type`, `credits`, `rate_limit_reset_credits`.
- 5 h vs weekly: classify by `limit_window_seconds`: 18000 = 5 h, 604800 (or 28-31 days) = weekly/monthly; fall back to `primary_window` = 5 h, `secondary_window` = weekly when seconds are missing. MC:codex/data.ts:66-71, 135-187. `reset_at` is a number (epoch, seconds assumed; unit parsing is in `utils/quota/resetInstants`, NOT READ) or string; `reset_after_seconds` is a relative fallback (:114-115). If `limit_reached` or `allowed==false` with no percent, MC shows 100% (:46).
```json
{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,
  "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":3600,"reset_at":1761480000},
  "secondary_window":{"used_percent":30,"limit_window_seconds":604800,"reset_after_seconds":200000,"reset_at":1761700000}}}
```
(illustrative values.)

### Gemini / Antigravity / others
- **Gemini (gemini-cli OAuth) quota: NOT FOUND** in MC (`src/features/quota/providers/` has only antigravity, claude, codex, devin, kimi, meta, plugin, xai). `gemini` appears in MC only as a badge colour (MC:src/utils/quota/constants.ts:13).
- Antigravity (Google): MC:src/features/quota/providers/antigravity/data.ts; constants MC:constants.ts:41, 67-96 (URLs/models not read in detail; per-model remaining fraction, not 5h/weekly). Marked out of scope for the plugin.
- Plugin quota (provider-defined): `GET /v8/management/plugins/<id>/quota`, `POST` to fetch, `DELETE` to reset (CPA:server_management_v8.go:64-66); `POST /v0/management/quota/fetch` with `{auth_index, provider?, plugin_id?}` and `GET /v0/management/quota/providers` (CPA:server_management.go:84-86; internal/api/handlers/management/plugin_quota.go:44-101). Response shape is plugin-defined (`pluginapi.QuotaFetchResponse`, NOT READ).
- Kimi `GET https://api.kimi.com/coding/v1/usages`, xAI billing URLs: MC:constants.ts:141-142, 155-160. Out of scope.

## 6. v0 equivalents (all under `/v0/management`, same key)

| v8 | v0 | Source |
| --- | --- | --- |
| `GET /credentials` | `GET /auth-files` | CPA:server_management.go:180 |
| `POST /requests/api-call` | `POST /api-call` | server_management.go:73 |
| `GET /observability/usage/api-keys` | `GET /api-key-usage` | :92 |
| `GET /observability/usage/queue` | `GET /usage-queue` | :93 |
| `POST /routing/cooldown/reset` | `POST /reset-quota` (`{"auth_index"}`) | :82; quota.go:27-70 |
| `GET /config/access/api-keys` | `GET /api-keys` | docs/management-api-v8.md:237-238 |
v0 payloads are unchanged from before v8 (CPA:docs/management-api-v8.md:228-231). MC probes `/v0/management/config` to detect legacy backends (MC:src/services/api/legacyBackendProbe.ts:17). Old CPA versions (pre-v8) may expose a `/v0/management/usage` aggregate with per-model counts; **NOT FOUND in this commit** (no such route), so it cannot be sourced here.

## 7. Plugin implications

1. Use v8 with fallback to v0 only for credentials (`/credentials` then `/auth-files`) and api-call, probing once (404 => try v0).
2. Management key is separate from client key; config needs both (`key`/`key_env` for client, a second one for management). Management key may be disabled (404) or remote-blocked (403): plugin must degrade to "no usage".
3. Merged bars need one api-call per credential (claude `five_hour`/`seven_day`, codex `primary/secondary`), 60 s cache as planned; each call goes upstream (a network call made by the proxy, on the user's own setup).
4. Credentials with `disabled`, `status != active`, or non-claude/codex provider: show count only, no bar.
5. Per-model lines (req + tokens): not available, see section 4; ask for a spec decision.

## Retry, limits, fallback (verified)
Source: CLIProxyAPI @ 3078035 (shallow clone). Paths are relative to the repo root (`CPA:`). Line numbers are from that commit.

### 1. 429 from an upstream credential: retry on another credential
- **Yes, same request, same provider/model pool.** In `executeMixedOnce` each failed credential goes into `tried`, then the loop picks the next credential (CPA:sdk/cliproxy/auth/conductor_execution.go:483-690; `tried[auth.ID]` at :526, `continue` at :652-666). The next pick excludes tried ids, so one request tries each eligible credential once. The same holds for streaming, count_tokens and Home mode (`executeStreamMixedOnce` :915, `executeCountMixedOnce` :697).
- **Rounds.** `Manager.Execute` wraps that in rounds (CPA:conductor_execution.go:122-179). After a round fails, `shouldRetryAfterErrorWithAttempted` decides on another round (CPA:conductor_selection.go:1370-1438). Retry applies only to 403/408/429/500/502/503/504 or transient transport errors (:1485-1505).
- **`routing.retry.request-retry`** (default 3 in the example) is the number of extra rounds after round 0. Round r only admits credentials whose effective request-retry is >= r. A per-credential or per-provider `request-retry` overrides it (0 = round 0 only, negative or omitted = inherit). Docs: CPA:config.example.yaml:187-198; `effectiveRequestRetryLimit` CPA:conductor_selection.go:1126-1134.
- **`max-retry-credentials`** (0 = all) caps how many different credentials are tried per round (CPA:config.example.yaml:200-204; check at conductor_execution.go:503).
- **`max-retry-interval`** (seconds, default 30) is the longest cooldown wait between rounds. If the soonest recovery is later than that, or the value is <= 0 and a wait is needed, the request stops without waiting (CPA:config.example.yaml:206-209; conductor_selection.go:1356-1438, `waitForCooldown` :1533). Same-round failover to the next credential never waits.
- **`routing.cooldown.*`:**
  - `disable-cooling` (default false).
  - `save-cooldown-status` (persist `.cds` files).
  - `transient-error-cooldown-seconds` (408/5xx/520-526; 0 = 60 s, -1 = off).
  - Docs: CPA:config.example.yaml:211-225.
- **A 429 cooldown.** The credential/model is marked cooling: `Retry-After` from upstream if present (floored at `minQuotaCooldownFloor`), else exponential backoff `quotaBackoffBase * 2^level` up to `quotaBackoffMax` (CPA:conductor_cooldown.go:1094-1126, 2808-2822).
- **`cooldown.disable-cooling: true`.**
  - No cooldown is written for the 429 (`next` stays zero; state cleared at CPA:conductor_cooldown.go:1229-1232). `nextQuotaCooldown` returns 0 (:2812).
  - Within one request nothing changes: the `tried` set still fails over to the next credential, and rounds retry immediately because no cooldown wait is found.
  - Across requests the limited credential is **not skipped**: it stays eligible and is picked again, so every new request can hit the same 429 first.
  - Per-credential and per-provider `disable-cooling` override the global value (CPA:conductor_cooldown.go:55-98).
  - In Home mode it is forced on (:57-59).
- **Session-affinity re-pin: yes.**
  - Needs `routing.session-affinity: true` (default false). The binding TTL is `session-affinity-ttl` (1h).
  - When the bound credential is unavailable the selector asks the fallback selector (round-robin/fill-first/weighted) for another one and calls `bind(auth.ID, true)`, which re-pins (CPA:sdk/cliproxy/auth/selector.go:1198-1221).
  - A 429/401/402/403/404 or model-unsupported failure counts as a rebind-worthy failure (`shouldRebindSessionAffinityOnError` :957-969).
  - `session-affinity-rebind-on: "any"` (default) rebinds on any credential-attributed failure. `"quota"` keeps the original binding across transient errors (408/5xx), serving that request from another credential without re-pinning, but still rebinds on 429/auth (CPA:config.example.yaml:176-182; selector.go:1217).
  - Caveat: with `disable-cooling: true` the 429 sets no cooldown, so the credential is not "unavailable" and the pin may stay on it. Only a recorded `lastErr` that matches the rebind rule makes it unavailable (selector.go:1007-1022). Not tested at runtime.

### 2. Every credential limited: what the client gets
- When all candidates are cooling, `getAvailableAuthsWithPriorityMode` returns a `modelCooldownError` (CPA:sdk/cliproxy/auth/selector.go:523-541).
- **Status 429.** `StatusCode()` returns `http.StatusTooManyRequests` (selector.go:349-351).
- **`Retry-After: <ceil seconds until the earliest recovery>`** and `Content-Type: application/json` (selector.go:353-362). The handler copies it onto the response (CPA:sdk/api/handlers/handlers_errors.go:126-129; Claude route sdk/api/handlers/claude/code_handlers.go:596-598).
- **Body** (selector.go:110-135):
  ```json
  {"error":{"code":"model_cooldown","message":"All credentials for model <m> are cooling down via provider <p> (last error: ...)","model":"<m>","reset_time":"1m30s","reset_seconds":90,"provider":"<p>","last_upstream_error":"..."}}
  ```
  `provider` and `last_upstream_error` appear only when known. The model name is restored to the one the client asked for (conductor_selection.go:708-716).
- This holds only when *every* credential is in cooldown and the retry rounds are exhausted. A single-credential failure with no one else to try returns that upstream error and status instead (`preferredExecutionAttemptError`, conductor_execution.go:167).
- Credentials that are disabled, unauthorized or expired give `auth_unavailable` / `auth_not_found` (`newAuthUnavailableError`, selector.go:541), not the cooldown body.

### 3. Model / cross-provider fallback: NO general mechanism
- Grep for `fallback-models`, `model-fallback`, `fallback_models` over Go, YAML and docs finds nothing (only an example plugin's "backendFallback" in CPA:examples/plugin/claude-web-search-router/go/main.go:77, unrelated).
- What exists is narrower:
  - **Model pool inside one alias** (api-key / openai-compat groups): repeat the same `alias` with different upstream `name`s. The alias round-robins across them, and "if the chosen upstream fails before producing output, the request continues with the next upstream model in the same alias pool" (CPA:config.example.yaml:906-918; conductor_stream.go:230 `executeStreamWithModelPool`). This is retry across upstream models behind one client-visible name, within the credential/provider that owns that alias. It is not a claude->gpt fallback unless you point an openai-compat group at a GPT model and name its alias like a Claude model.
  - **Provider pool for one model**: `Execute` takes `providers []string` from `util.GetProviderName(model)` (CPA:sdk/api/handlers/handlers_routing.go:201), and `executeMixedOnce` picks across them ("mixed"). If the same model id is served by several providers they are tried in turn. A different model never is.
  - Antigravity credits fallback (`tryAntigravityCreditsExecute`, conductor_execution.go:169) is Antigravity-only.
  - Plugins (`internal/pluginhost`) can supply custom schedulers, but nothing built in does model fallback.
- So an ai-launcher "Claude limited -> GPT" feature must be done by the client (pick another model/alias) or by an alias pool. The proxy will not do it on its own.

### 4. Claude Code asking for a GPT model through /v1/messages
- **A real id works, no alias needed.** `POST /v1/messages` (CPA:internal/api/server_routes.go:84) goes to `ClaudeMessages`, which resolves providers purely from the model name via the registry: `util.GetProviderName(baseModel)` (CPA:sdk/api/handlers/handlers_routing.go:165-210). If any registered provider owns `gpt-5-codex` (a Codex OAuth credential, or an openai-compat/api-key group listing it), the request routes there. The format translator is chosen by the source (claude) and target (codex or openai) pair (CPA:internal/translator/codex/claude/, CPA:internal/translator/openai/claude/).
- Unknown model: HTTP 400 `{"error":{"type":"invalid_request_error","code":"model_not_found","param":"model","message":"unknown provider for model <m>"}}` (handlers_routing.go:210-229).
- **Model ids that Claude Code accepts.** Claude Code itself may reject or alter odd ids client-side (not verified here). Aliases solve that: expose a Claude-looking name.
- **Aliases, `oauth.model-alias`** (CPA:config.example.yaml:927-975):
  - Per channel (vertex, aistudio, antigravity, claude, codex, kimi, xai, meta). Each entry is `{name: <upstream id>, alias: <client-visible id>}`. The same name can have several aliases.
  - Optional fields: `fork: true` (also keep the original id visible), `display-name`, `force-mapping: true` (rewrite the response's model field back to the alias).
  - The table is built in `compileOAuthModelAliasTable` (CPA:sdk/cliproxy/auth/oauth_model_alias.go:34-72). The alias is looked up case-insensitively, first entry wins, and name==alias is skipped.
  - Per-auth: `"model_aliases": [{name, alias}]` inside the OAuth JSON, which beats global aliases for that credential.
  - **Does not apply to `api-keys` groups.** Those have their own `models: [{name, alias}]` (CPA:config.example.yaml:895-918).
  - Example: codex channel `{name: gpt-5-codex, alias: claude-sonnet-4-5}` makes a Claude Code request for that Claude id execute on Codex. The model merges into one /v1 list, so a name that overlaps another provider is ambiguous (note at :934-936).

### 5. `routing.strategy` values and ordering
Valid values (CPA:internal/config/config_types.go:418-420, normaliser CPA:internal/api/handlers/management/config_basic.go:312-324):
- `round-robin` (default; aliases `roundrobin`, `rr`; empty also means this)
- `weighted-round-robin` (`weightedroundrobin`, `wrr`)
- `fill-first` (`fillfirst`, `ff`)

Any other value is rejected with 400 `invalid strategy` by the management PUT. There is no `priority` strategy; priority is a per-credential field that works under all three.

| Strategy | Pick per request | Sticks until limited? | Source |
| --- | --- | --- | --- |
| round-robin | Next credential after the previous pick, in credential-ID order, keyed by `provider:model`. Wraps around. | No, rotates every request. | CPA:sdk/cliproxy/auth/selector.go:614-654, `successorIndex` :633 |
| weighted-round-robin | Smooth weighted round-robin over per-credential `weight`. Weights <= 0 exclude the credential. | No, shares traffic by weight. | CPA:selector.go:673-800; weights doc config.example.yaml:151-153 |
| fill-first | Always the first available credential (lowest ID in the top priority tier). | **Yes, true failover.** It keeps hitting that credential until it is blocked (cooling, disabled, quota), then the next one. When the first recovers, it is chosen again for new requests. | CPA:selector.go:812-822 (`available[0]`), scheduler CPA:sdk/cliproxy/auth/scheduler.go:1383 |

- **Ordering and priority.** Per-credential integer `priority` (larger wins; default 0), read from `Attributes["priority"]` (CPA:selector.go:364-377). Each selector only sees the highest available priority tier (`highestPriorityAuths` :585-610). Lower tiers are used only when every higher-tier credential is unavailable. Set it in the credential's config: `priority:` on every api-key / openai-compat entry (CPA:config.example.yaml:521) and as a `priority` field in an OAuth auth JSON (editable through `PATCH /v8/management/credentials/fields`, TUI Auth tab "Priority"). Inside one tier, order is credential id (sorted), not file order. There is no ordered-list setting.
- **Session-affinity interaction.** The affinity selector wraps the configured strategy as its `fallback`. An established binding (credential kept for the session) beats priority and the strategy. The configured strategy decides only (a) the first binding of a new session, (b) requests with no session id, and (c) the credential chosen when the bound one becomes unavailable, which is then re-pinned (CPA:selector.go:912-918, 1198-1221; docs config.example.yaml:154-165). With `fill-first` plus affinity, a session stays on its credential even after a higher-priority one recovers. New sessions go to the first available.
- **Changing it at runtime.**
  - v8: `PUT https://<host>/v8/management/config/routing/strategy` with `Authorization: Bearer <management key>` (or `X-Management-Key`) and a JSON body that is the bare value: `"fill-first"`. The v8 docs say "JSON writes accept the value directly, without a `{ "value": ... }` envelope", `PUT` replaces the target, and `PATCH /v8/management/config` with `{"routing":{"strategy":"fill-first"}}` also works (CPA:docs/management-api-v8.md:26-70; routes CPA:internal/api/server_management_v8.go:23-30 catch-all `/config/*path`). The exact v8 handler for a scalar path was not run; the doc example uses `routing.retry.request-retry` the same way.
  - v0 (older, also served): `PUT /v0/management/routing/strategy` (PATCH too) with body `{"value":"fill-first"}`, 400 `invalid body` if `value` is missing (CPA:internal/api/server_management.go:131-133; config_basic.go:335-349). `GET` returns `{"strategy":"round-robin"}`.
  - Config file is hot-reloaded by the watcher (`routing.strategy` diff at CPA:internal/watcher/diff/config_diff.go:198), so editing the YAML also works.

### 6. Routing GET / config PATCH / v0 strategy, as the plugin uses them (verified)
Source: CLIProxyAPI 3078035.
- **Routes.** v8 `PATCH /config` and `GET|PUT|PATCH|DELETE /config/*path` all go to `ConfigV8` (CPA:internal/api/server_management_v8.go:23-27). v0 `GET /routing/strategy`, `PUT` and `PATCH` (CPA:internal/api/server_management.go:131-133), handler `PutRoutingStrategy` (CPA:internal/api/handlers/management/config_basic.go:335).
- **GET `/v8/management/config/routing`** returns the YAML subtree as JSON, after projecting the v8 aliases (CPA:internal/api/handlers/management/config_v8.go:58-85). Missing path → 404 `not_found`. A key absent from the YAML file is absent from the answer, so the plugin reads a missing `strategy` as `round-robin` and a missing `session-affinity-subagents` as true (its documented default).
- **PATCH `/v8/management/config`** merges the JSON body into the config (`mergeConfigV8Patch`, config_v8.go:140, :360). No `If-Match` header means no precondition (`ifMatchSatisfied`, config_v8.go:226-229). Lists are replaced whole; maps merge.
- **Keys the plugin reads and writes** (all exist):
  - `routing.strategy` (CPA:internal/config/config_types.go:420, json `strategy`, omitempty)
  - `routing.session-affinity` (:427), `routing.session-affinity-ttl` (:431), `routing.session-affinity-subagents` (:438, `*bool`, default true per :436)
  - `routing.cooldown.disable-cooling`: stored as top-level `disable-cooling` (CPA:internal/config/config.go:83); the v8 layout maps it to `routing.cooldown.disable-cooling` (CPA:internal/config/config_v8.go:81), which is what GET shows and PATCH accepts. Example config: CPA:config.example.yaml:211-214.
- **Plugin bodies.** failover: `{"routing":{"strategy":"fill-first","cooldown":{"disable-cooling":false}}}`. balance: `{"routing":{"strategy":"round-robin" (or "weighted-round-robin" with `weights=on`),"session-affinity":true,"session-affinity-subagents":true[,"session-affinity-ttl":"1h" when none is set]}}`. affinity: the same minus `strategy`.
- **v0 fallback** (v8 answers 404): `PUT /v0/management/routing/strategy` with `{"value":"fill-first"}` (or round-robin / weighted-round-robin); `GET` returns `{"strategy":"..."}` (config_basic.go:312-349). v0 has no way used here to set affinity or cooling, so only the strategy changes.
- **Per-credential weight** (used by `weighted-round-robin`). An int, default 1 when absent, max 1,000,000; 0 or less excludes the credential from picking. It is a top-level `weight` field in each entry of the credentials / auth-files list. Set it with `PATCH /v8/management/credentials/fields` body `{"name":"<auth file name or id>","weight":N}` (v0: `PATCH /v0/management/auth-files/fields`, same body); it applies without a restart. The plugin keeps it to 0..99, finds the account by `auth_index`, sends its `name` (else `id`), tries v8 first and v0 on 404. With session affinity the weight only chooses the account for a new chat; a bound chat stays where it is.
- **Strategy aliases.** `rr`/`roundrobin`, `wrr`/`weightedroundrobin`, `ff`/`fillfirst` (config_basic.go:312-324); the plugin normalises them the same way before comparing.
