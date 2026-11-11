# PandaCode Connect relay

A Cloudflare Worker with one SQLite-backed Durable Object per account. It signs devices in with
GitHub, keeps the device list, and forwards end-to-end encrypted frames between an account's
devices. It stores no task content. The contract is in [docs/remote-access.md](../docs/remote-access.md).

## Deploy

1. Create a GitHub OAuth App (Settings → Developer settings → OAuth Apps) and tick **Enable Device Flow**.
   Generate a client secret too: the relay uses it only to check that a sign-in token was issued to this app (`POST /applications/{client_id}/token`). The desktop app never sees it. The client ID also goes into the desktop build as `PANDACODE_GITHUB_CLIENT_ID`.
2. From this folder:
   ```sh
   npm install
   openssl rand -base64 48 | npx wrangler secret put SESSION_SECRET
   npx wrangler secret put GITHUB_CLIENT_ID
   npx wrangler secret put GITHUB_CLIENT_SECRET
   npx wrangler deploy
   ```
   Each `secret put` takes the secret's name and asks for its value (paste the OAuth App's client ID,
   then its client secret). Run them one line at a time.
   The deploy creates the `Account` Durable Object (SQLite migration `v1`) and the `AUTH_LIMITER` (10/min per IP) `ACCOUNT_LIMITER` (30/min per account) and `GLOBAL_AUTH_LIMITER` (300/min across all sign-ins, checked after the per-IP one) rate-limit bindings.
3. Build the desktop app with `PANDACODE_RELAY_URL` set to the Worker URL (or set it as an environment variable at run time).

Rotating `SESSION_SECRET` signs everyone out. `SESSION_SECRET` must be at least 32 characters; with it missing or shorter (or the GitHub ID/secret missing, for sign-in) every request gets a generic `500 server error`.

Hardening notes:
- Sign-in bodies are read through a streaming cap of 256 KiB (`src/body.ts`), so chunked uploads get 413 too.
- Tokens carry `gen`, a random value stored on the device row when it is first registered. The Durable Object rejects a token whose `gen` differs from the row's, so a revoke followed by a re-register in the same second kills old tokens. A plain re-sign-in of a still-registered device keeps its `gen` (its other tokens stay valid; same device, same key). Tokens without `gen` (issued before schema 2) fall back to the `iat >= registered_at` check; `/v1/token` upgrades them. Schema 2 adds `devices.gen` (existing rows get 0).
- A rate-limited WebSocket frame over 4 KiB is answered `rate limit exceeded` without parsing it.
- **Push** (`src/push.ts`): a signed-in phone can register a device push token with
  `POST /v1/push/devices` (`{ platform: "ios" | "android", token }`; one device keeps up to
  3 per platform). A host with remote control on can send a `{ kind: "notify", body: { kind:
  "waiting" | "finished", count } }` frame on its socket; the object coalesces bursts (30 s
  window, halved when the waiting count grows) and forwards the notice to every registered
  token via APNs/FCM. Payloads carry counts and a deep link, never task text. The bridge is
  env-gated: with none of `APNS_KEY`/`APNS_KEY_ID`/`APNS_TEAM_ID`/`APNS_TOPIC` (iOS, the .p8
  provider key) or `FCM_SERVICE_ACCOUNT`/`FCM_PROJECT_ID` (Android) set, registrations are
  stored and notify frames answered, but nothing is sent.

## Behaviour

- `GET /v1/connect?host=0|1` opens the socket and sets the host flag and `last_seen` in one write, then broadcasts presence once. A `{kind:"host"}` frame still works; repeating the stored value writes and broadcasts nothing.
- `POST /v1/token` (Bearer relay token) returns `{ token }`: a fresh 30-day token for the same account and device, `iat` = now. It passes the same checks as `/v1/devices` (signature and expiry, `ACCOUNT_LIMITER`, device still registered, `iat` not before registration), so a revoked device gets 401 and cannot refresh. Other failures are the usual 401/429 JSON errors; other methods get 405.
- Frames are `{ to, body, ref? }` (`ref`: opaque, at most 64 chars). When a frame can't go out, the sender gets `{ kind: "undeliverable", to, ref?, reason: "offline" | "rate" }` with `ref` echoed. Malformed frames get `{ kind: "error", error }`.
- Presence is `{ kind: "presence", devices: [{ id, name, platform, publicKey, host, online, lastSeen }] }`, the full registered list, the same shape as `GET /v1/devices`.
- Clients send `ping` every 30 s (answered by the runtime without waking the object). A socket is online only while its last ping, or its join before the first ping, is under 75 s old; stale sockets are closed (code 4002) when found and presence is broadcast.
- A reconnect keeps the device's rate bucket (20/s, burst 100), seeded from the old socket before it closes.
- The schema migrates once, gated on `PRAGMA user_version`.

## Self-hosted mode (Quick connect)

The same Worker also runs on a user's own Cloudflare account, deployed by the desktop app
(`src-tauri/src/quick_connect/`) from `wrangler.self.json`: `RELAY_MODE=self`, no custom
domain, no GitHub app, no rate-limit bindings. One Durable Object (`self`) holds the device
list. Frames, presence, encryption and every authenticated `/v1/*` route are unchanged; only
how a device gets its token differs (`src/self.ts`, contract in
[docs/remote-access.md](../docs/remote-access.md#quick-connect)):

- `HOST_SECRET` (a Worker secret, at least 32 characters, stored by the desktop right after the
  deploy) replaces GitHub sign-in. Until it is stored every request gets `500 server error`.
  It only authenticates the desktop. Device tokens are signed with a key the deployment's
  Durable Object makes once and keeps (`self:signing-key`), so rotating the host secret leaves
  every paired phone paired.
- A paired device stays paired for 30 days from pairing (`SELF_DEVICE_TTL_SECONDS`, the lifetime
  of a ViewCode session). Use and `POST /v1/token` never extend it: a refreshed token ends at the
  same moment. When it lapses the row is deleted and its socket closed (`4001`), exactly as a
  revoke; the phone pairs again with a new code. The desktop's own row has no fixed end.
- `GET /v1/self/probe`, `POST /v1/self/host`, `POST|DELETE /v1/self/pairings` take
  `Authorization: Bearer <HOST_SECRET>`. `host` registers the desktop as the deployment's
  admin device; `pairings` offers `{ codeHash, ttlSeconds ≤ 300 }` or cancels the waiting ones.
- `POST /v1/self/pair` `{ code, device }` needs no account: the SHA-256 of `code` must match a
  waiting pairing, which is then spent. Unknown, used and expired codes get the same `401`.
- Only the admin device hosts, removes other devices or deletes the deployment's state; a
  pairing never registers under the admin device's id. `/v1/auth/github` is `404` here, and
  `/v1/self/*` is `404` on the shared relay.

## Develop

```sh
npm run typecheck   # tsc for the Worker and for the tests
npm test            # node --test (needs Node 22.18+ for TypeScript stripping)
npx wrangler dev    # local; put SESSION_SECRET=... in .dev.vars
```

| File | Job |
|---|---|
| `src/index.ts` | Routes, auth rate limits, GitHub sign-in, token check |
| `src/self.ts` | Self-hosted mode: host secret, pairing rules, its routes (pure; `test/self.test.ts`) |
| `src/account.ts` | Durable Object: device rows, hibernating WebSockets, forwarding, presence |
| `src/token.ts`, `body.ts`, `config.ts`, `policy.ts`, `validate.ts`, `bucket.ts`, `frames.ts`, `github.ts`, `limits.ts` | Pure logic, unit tested |
