import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { deleteSelfState, SIGNING_KEY } from "../src/account-delete.ts";
import { SELF_DEVICE_TTL_SECONDS, TOKEN_TTL_SECONDS } from "../src/limits.ts";
import { tokenCurrent } from "../src/policy.ts";
import { isKnownRoute } from "../src/routes.ts";
import {
  credentialExpired, deviceExpiry, handleSelf as route, hostAuthorized, isPairingCode, isSigningKey, MAX_PENDING_PAIRINGS, mayAdminister, mayPairAs, mayRevoke,
  newSigningKey, offerPairing, PAIRING_TTL_MS, pairingVerdict, parsePairingOffer, redeemPairing, relayMode, selfConfigured, sha256Hex, signingKey, tokenLifetime,
  type AccountFetch, type KeyCache, type PairingRow, type PairingStore, type SelfEnv,
} from "../src/self.ts";
import { verifyToken } from "../src/token.ts";

const HOST_SECRET = "h".repeat(43);
const ENV = { RELAY_MODE: "self", HOST_SECRET };
const HOST_ID = "3f2b8c1e-9a4d-4e0b-8c55-0a1b2c3d4e5f";
const PHONE_ID = "7a1c9d2e-5b6f-4a3b-9c8d-1e2f3a4b5c6d";
const key = (fill: number) => btoa(String.fromCharCode(...new Uint8Array(32).fill(fill)));
const hostDevice = { id: HOST_ID, name: "Mac", platform: "macos", publicKey: key(1) };
const phone = { id: PHONE_ID, name: "Phone", platform: "ios", publicKey: key(2) };
const vector = JSON.parse(readFileSync(new URL("./fixtures/quick-connect-pairing.json", import.meta.url), "utf8")) as {
  token: string; relayCode: string; relayCodeHash: string; hostCode: string;
};

function memoryStore(): PairingStore & { rows: Map<string, PairingRow> } {
  const rows = new Map<string, PairingRow>();
  return {
    rows,
    get: (hash) => rows.get(hash),
    insert: (row) => void rows.set(row.code_hash, { ...row }),
    markUsed: (hash, at) => { const row = rows.get(hash); if (row) row.used_at = at; },
    prune: (before) => { for (const [hash, row] of rows) if (row.expires_at < before) rows.delete(hash); },
    pending: (now) => [...rows.values()].filter((row) => row.used_at === null && row.expires_at > now).length,
    cancel: () => { for (const [hash, row] of rows) if (row.used_at === null) rows.delete(hash); },
  };
}

/** The Durable Object's part of the self-hosted routes, over the same pure rules it uses. */
function fakeAccount(clock: { now: number }) {
  const store = memoryStore();
  const devices = new Map<string, { admin: number; public_key: string; gen: number; expires_at: number }>();
  const seen: string[] = [];
  // Made once, like the object's stored key; nothing in a request can change it.
  const key = newSigningKey();
  let gen = 100;
  const reply = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
  const register = (device: { id: string; publicKey: string }, admin: number, expiresAt = 0) => {
    const existing = devices.get(device.id);
    if (existing) existing.expires_at = expiresAt;
    else devices.set(device.id, { admin, public_key: device.publicKey, gen: gen++, expires_at: expiresAt });
    return reply({ ok: true, gen: devices.get(device.id)!.gen, expiresAt });
  };
  const fetch = async (request: Request): Promise<Response> => {
    const path = new URL(request.url).pathname;
    if (path === "/internal/self/key") return reply({ key });
    seen.push(`${request.method} ${path}`);
    const nowS = Math.floor(clock.now / 1000);
    for (const [id, row] of devices) if (credentialExpired(row, nowS)) devices.delete(id);
    if (path === "/internal/self/host") return register(await request.json(), 1);
    if (path === "/internal/self/pairings") {
      if (request.method === "DELETE") { store.cancel(); return reply({ ok: true }); }
      const error = offerPairing(store, (await request.json()) as PairingRow, clock.now);
      return error ? reply({ error }, 409) : reply({ ok: true });
    }
    if (path === "/internal/self/pair") {
      const { codeHash, device } = (await request.json()) as { codeHash: string; device: { id: string; publicKey: string } };
      if (!mayPairAs(devices.get(device.id))) return reply({ error: "device key changed" }, 409);
      if (redeemPairing(store, codeHash, clock.now) !== "ok") return reply({ error: "pairing code is not valid or has expired" }, 401);
      return register(device, 0, deviceExpiry(nowS));
    }
    // An authenticated route: the Worker has already checked the token and named the device.
    const row = devices.get(request.headers.get("x-device-id") ?? "");
    if (!row || !tokenCurrent({ iat: Number(request.headers.get("x-token-iat")), gen: Number(request.headers.get("x-token-gen")) }, { gen: row.gen, registered_at: 0 })) {
      return reply({ error: "device is not registered" }, 401);
    }
    return reply({ ok: true, gen: row.gen, expiresAt: row.expires_at, authorization: request.headers.get("authorization") });
  };
  return { fetch, store, devices, seen, key, keys: {} as KeyCache };
}

/** `handleSelf` with a key cache per deployment (per fake account), as each isolate has one. */
const caches = new WeakMap<AccountFetch, KeyCache>();
function handleSelf(request: Request, env: SelfEnv, account: AccountFetch, nowMs?: number): Promise<Response> {
  if (!caches.has(account)) caches.set(account, {});
  return route(request, env, account, nowMs, caches.get(account));
}

const call = (path: string, init: { method?: string; secret?: string | null; token?: string; body?: unknown } = {}) =>
  new Request(`https://relay.example${path}`, {
    method: init.method ?? (init.body === undefined ? "GET" : "POST"),
    headers: init.token ? { authorization: `Bearer ${init.token}` } : init.secret === null || init.secret === undefined ? {} : { authorization: `Bearer ${init.secret}` },
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });
const body = async (res: Response) => (await res.json()) as Record<string, any>;

test("mode: shared unless RELAY_MODE is self, and a self deployment needs its secret first", async () => {
  assert.equal(relayMode({}), "shared");
  assert.equal(relayMode({ RELAY_MODE: "self" }), "self");
  assert.equal(selfConfigured({ RELAY_MODE: "self" }), false);
  assert.equal(selfConfigured({ RELAY_MODE: "self", HOST_SECRET: "short" }), false);
  assert.equal(selfConfigured(ENV), true);
  // Deployed, secret not stored yet: every route answers the same generic 500, before anything is asked of the object.
  const account = fakeAccount({ now: 0 });
  for (const path of ["/v1/self/probe", "/v1/self/pair", "/v1/devices", "/v1/auth/github"]) {
    const res = await handleSelf(call(path, { secret: HOST_SECRET }), { RELAY_MODE: "self" }, account.fetch);
    assert.deepEqual([res.status, await body(res)], [500, { error: "server error" }]);
  }
  assert.deepEqual(account.seen, []);
  // The shared relay never serves these paths: its router only forwards known routes.
  for (const path of ["/v1/self/probe", "/v1/self/host", "/v1/self/pair", "/v1/self/pairings", "/internal/self/host"]) assert.equal(isKnownRoute(path), false);
});

test("host secret: good, bad and missing", async () => {
  assert.equal(await hostAuthorized(HOST_SECRET, `Bearer ${HOST_SECRET}`), true);
  for (const bad of [null, "", HOST_SECRET, `Bearer ${HOST_SECRET}x`, `Bearer ${HOST_SECRET.slice(1)}`, `Basic ${HOST_SECRET}`, `Bearer ${"h".repeat(600)}`]) {
    assert.equal(await hostAuthorized(HOST_SECRET, bad), false);
  }
  const account = fakeAccount({ now: 0 });
  const ok = await handleSelf(call("/v1/self/probe", { secret: HOST_SECRET }), ENV, account.fetch);
  assert.deepEqual([ok.status, await body(ok)], [200, { ok: true, mode: "self", protocol: 1 }]);
  for (const secret of ["x".repeat(43), null]) {
    for (const [path, method] of [["/v1/self/probe", "GET"], ["/v1/self/host", "POST"], ["/v1/self/pairings", "POST"], ["/v1/self/pairings", "DELETE"], ["/v1/self/nope", "GET"]]) {
      const res = await handleSelf(call(path, { secret, method, body: method === "POST" ? { device: hostDevice } : undefined }), ENV, account.fetch);
      assert.deepEqual([res.status, await body(res)], [401, { error: "invalid host secret" }]);
    }
  }
  // Nothing reached the Durable Object without the secret, and GitHub sign-in does not exist here.
  assert.deepEqual(account.seen, []);
  assert.equal((await handleSelf(call("/v1/auth/github", { body: {} }), ENV, account.fetch)).status, 404);
  assert.equal((await handleSelf(call("/v1/self/nope", { secret: HOST_SECRET }), ENV, account.fetch)).status, 404);
});

test("the host registers with the secret and gets a token signed with the derived key", async () => {
  const account = fakeAccount({ now: 0 });
  const res = await handleSelf(call("/v1/self/host", { secret: HOST_SECRET, body: { device: hostDevice } }), ENV, account.fetch, 5_000_000);
  const reply = await body(res);
  assert.equal(res.status, 200);
  assert.deepEqual(reply.account, { id: "self", login: "", avatarUrl: "" });
  // The desktop's own token is an ordinary 30-day one; it can always register again with the secret.
  assert.deepEqual(await verifyToken(account.key, reply.token, 5001), { acct: "self", dev: HOST_ID, iat: 5000, exp: 5000 + TOKEN_TTL_SECONDS, gen: 100 });
  // The host secret is not the signing key, and another deployment's key does not verify it.
  assert.equal(await verifyToken(HOST_SECRET, reply.token, 5001), null);
  assert.equal(await verifyToken(fakeAccount({ now: 0 }).key, reply.token, 5001), null);
  assert.equal(account.devices.get(HOST_ID)?.admin, 1);
  assert.equal((await handleSelf(call("/v1/self/host", { secret: HOST_SECRET, body: { device: { ...hostDevice, publicKey: "no" } } }), ENV, account.fetch)).status, 400);
});

test("pairing: one use, five minutes, and the relay never stores the code itself", async () => {
  const clock = { now: 1_000_000 };
  const account = fakeAccount(clock);
  const offer = (codeHash: string, ttlSeconds = 300) => handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, body: { codeHash, ttlSeconds } }), ENV, account.fetch, clock.now);
  const pair = (code: string, device = phone) => handleSelf(call("/v1/self/pair", { body: { code, device } }), ENV, account.fetch, clock.now);

  assert.equal(await sha256Hex(vector.relayCode), vector.relayCodeHash);
  assert.equal((await offer(vector.relayCodeHash)).status, 200);
  assert.deepEqual([...account.store.rows.keys()], [vector.relayCodeHash]);

  // A wrong or malformed code, and the pairing token or the host's code in its place, all get the same refusal.
  for (const code of ["A".repeat(43), "short", vector.hostCode, vector.token, 7]) {
    const res = await pair(code as string);
    assert.deepEqual([res.status, await body(res)], [401, { error: "pairing code is not valid or has expired" }]);
  }
  // The right code pairs the phone without any account, as a non-admin device.
  const paired = await pair(vector.relayCode);
  const reply = await body(paired);
  assert.equal(paired.status, 200);
  const claims = await verifyToken(account.key, reply.token, 1001);
  assert.deepEqual([claims?.acct, claims?.dev, claims?.exp], ["self", PHONE_ID, 1000 + SELF_DEVICE_TTL_SECONDS]);
  assert.equal(account.devices.get(PHONE_ID)?.admin, 0);

  // Replay of the used code, by the same phone or another device.
  for (const device of [phone, { ...phone, id: "9b2d0e3f-6c7a-4b4c-8d9e-2f3a4b5c6d7e" }]) {
    const replay = await pair(vector.relayCode, device);
    assert.deepEqual([replay.status, await body(replay)], [401, { error: "pairing code is not valid or has expired" }]);
  }
  assert.equal(account.devices.size, 1);
  // The same hash cannot be offered again to revive it.
  assert.equal((await offer(vector.relayCodeHash)).status, 409);

  // Expired: offered for 60 s, redeemed a second too late.
  const late = "B".repeat(43);
  assert.equal((await offer(await sha256Hex(late), 60)).status, 200);
  clock.now += 60_000;
  assert.equal((await pair(late)).status, 401);
  assert.equal(account.devices.size, 1);

  // Offers: bounded lifetime, a real digest, and a cap on how many wait.
  for (const bad of [{ codeHash: "x", ttlSeconds: 60 }, { codeHash: vector.relayCodeHash, ttlSeconds: 301 }, { codeHash: vector.relayCodeHash, ttlSeconds: 0 }, { codeHash: vector.relayCodeHash }]) {
    assert.equal((await handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, body: bad }), ENV, account.fetch, clock.now)).status, 400);
  }
  for (let i = 0; i < MAX_PENDING_PAIRINGS; i++) assert.equal((await offer(await sha256Hex(`pending-${i}`))).status, 200);
  assert.equal((await offer(await sha256Hex("one-too-many"))).status, 409);
  // Cancelling drops the waiting ones and keeps used rows, so a replay still fails.
  assert.equal((await handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, method: "DELETE" }), ENV, account.fetch, clock.now)).status, 200);
  assert.equal(account.store.pending(clock.now), 0);
  assert.equal(pairingVerdict(account.store.get(vector.relayCodeHash), clock.now), "used");
  assert.equal((await pair(vector.relayCode)).status, 401);
});

test("pairing rules on their own", () => {
  const store = memoryStore();
  const row = parsePairingOffer({ codeHash: "a".repeat(64), ttlSeconds: 300 }, 1000) as PairingRow;
  assert.deepEqual(row, { code_hash: "a".repeat(64), expires_at: 1000 + PAIRING_TTL_MS, used_at: null });
  assert.equal(redeemPairing(store, row.code_hash, 1000), "unknown");
  assert.equal(offerPairing(store, row, 1000), null);
  assert.equal(pairingVerdict(store.get(row.code_hash), row.expires_at), "expired");
  assert.equal(redeemPairing(store, row.code_hash, row.expires_at - 1), "ok");
  assert.equal(redeemPairing(store, row.code_hash, row.expires_at - 1), "used");
  assert.equal(isPairingCode("a".repeat(43)), true);
  assert.equal(isPairingCode("a".repeat(44)), false);
  // Old rows go once they can no longer matter.
  offerPairing(store, { code_hash: "b".repeat(64), expires_at: 10_000_000_000, used_at: null }, row.expires_at + 3_600_001);
  assert.deepEqual([...store.rows.keys()], ["b".repeat(64)]);
});

test("a paired device's token works until the device is revoked", async () => {
  const clock = { now: 2_000_000 };
  const account = fakeAccount(clock);
  await handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, body: { codeHash: vector.relayCodeHash, ttlSeconds: 300 } }), ENV, account.fetch, clock.now);
  const { token } = await body(await handleSelf(call("/v1/self/pair", { body: { code: vector.relayCode, device: phone } }), ENV, account.fetch, clock.now));

  const listed = await handleSelf(call("/v1/devices", { token }), ENV, account.fetch, clock.now);
  // The bearer token stops at the Worker; the Durable Object sees only the device it names.
  assert.deepEqual([listed.status, (await body(listed)).authorization], [200, null]);
  const refreshed = await handleSelf(call("/v1/token", { token, method: "POST" }), ENV, account.fetch, clock.now + 5000);
  assert.equal((await verifyToken(account.key, (await body(refreshed)).token, 2006))?.iat, 2005);

  // Bad, missing and foreign tokens never reach the Durable Object.
  const before = account.seen.length;
  for (const bad of [undefined, "nope", HOST_SECRET, `${token}x`]) assert.equal((await handleSelf(call("/v1/devices", { token: bad }), ENV, account.fetch, clock.now)).status, 401);
  assert.equal(account.seen.length, before);

  // Revoked: the row is gone, so the still-unexpired token is refused, and so is refreshing it.
  account.devices.delete(PHONE_ID);
  assert.equal((await handleSelf(call("/v1/devices", { token }), ENV, account.fetch, clock.now)).status, 401);
  assert.equal((await handleSelf(call("/v1/token", { token, method: "POST" }), ENV, account.fetch, clock.now)).status, 401);
  // Pairing again makes a new row with a new gen: the old token stays dead.
  assert.equal(tokenCurrent({ iat: 2000, gen: 101 }, { gen: 102, registered_at: 0 }), false);
});

test("only the host computer administers a self-hosted deployment", () => {
  const admin = { id: HOST_ID, admin: 1 };
  const paired = { id: PHONE_ID, admin: 0 };
  assert.equal(mayRevoke("self", admin, PHONE_ID), true);
  assert.equal(mayRevoke("self", paired, HOST_ID), false);
  assert.equal(mayRevoke("self", paired, PHONE_ID), true);
  assert.equal(mayRevoke("shared", paired, HOST_ID), true);
  assert.equal(mayAdminister("self", paired), false);
  assert.equal(mayAdminister("self", admin), true);
  assert.equal(mayAdminister("shared", paired), true);
  // A pairing code never lets a device register under the desktop's id.
  assert.equal(mayPairAs({ admin: 1 }), false);
  assert.equal(mayPairAs({ admin: 0 }), true);
  assert.equal(mayPairAs(undefined), true);
});

test("rotating the host secret keeps every phone paired", async () => {
  const clock = { now: 3_000_000 };
  const account = fakeAccount(clock);
  const NEW_SECRET = "n".repeat(43);
  const rotated = { RELAY_MODE: "self", HOST_SECRET: NEW_SECRET };
  await handleSelf(call("/v1/self/host", { secret: HOST_SECRET, body: { device: hostDevice } }), ENV, account.fetch, clock.now);
  await handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, body: { codeHash: vector.relayCodeHash, ttlSeconds: 300 } }), ENV, account.fetch, clock.now);
  const { token } = await body(await handleSelf(call("/v1/self/pair", { body: { code: vector.relayCode, device: phone } }), ENV, account.fetch, clock.now));

  // `wrangler secret put HOST_SECRET` with a new value: new isolates, the same Durable Object.
  caches.delete(account.fetch);
  // The old host secret is refused everywhere it used to work.
  for (const [path, method] of [["/v1/self/probe", "GET"], ["/v1/self/host", "POST"], ["/v1/self/pairings", "POST"]]) {
    assert.equal((await handleSelf(call(path, { secret: HOST_SECRET, method, body: method === "POST" ? { device: hostDevice } : undefined }), rotated, account.fetch, clock.now)).status, 401);
  }
  // The phone's credential is untouched: same token, still accepted, still refreshable, same end.
  assert.equal((await handleSelf(call("/v1/devices", { token }), rotated, account.fetch, clock.now)).status, 200);
  const refreshed = await body(await handleSelf(call("/v1/token", { token, method: "POST" }), rotated, account.fetch, clock.now + 1000));
  assert.equal((await verifyToken(account.key, refreshed.token, 3002))?.exp, 3000 + SELF_DEVICE_TTL_SECONDS);
  // The desktop comes back with the new secret and keeps its row; the phone is still there.
  const host = await handleSelf(call("/v1/self/host", { secret: NEW_SECRET, body: { device: hostDevice } }), rotated, account.fetch, clock.now);
  assert.equal(host.status, 200);
  assert.deepEqual([...account.devices.keys()].sort(), [HOST_ID, PHONE_ID].sort());
  assert.equal((await handleSelf(call("/v1/devices", { token }), rotated, account.fetch, clock.now)).status, 200);

  // Revoking is still immediate after a rotation.
  account.devices.delete(PHONE_ID);
  assert.equal((await handleSelf(call("/v1/devices", { token }), rotated, account.fetch, clock.now)).status, 401);
});

test("a pairing lasts 30 days from pairing; using or refreshing it never extends it", async () => {
  const day = 24 * 60 * 60 * 1000;
  const clock = { now: 10 * day };
  const paired = Math.floor(clock.now / 1000);
  const account = fakeAccount(clock);
  const offer = async (code: string) => handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, body: { codeHash: await sha256Hex(code), ttlSeconds: 300 } }), ENV, account.fetch, clock.now);
  await offer(vector.relayCode);
  const { token } = await body(await handleSelf(call("/v1/self/pair", { body: { code: vector.relayCode, device: phone } }), ENV, account.fetch, clock.now));
  assert.equal(SELF_DEVICE_TTL_SECONDS, 30 * 24 * 60 * 60);
  assert.equal((await verifyToken(account.key, token, paired))?.exp, paired + SELF_DEVICE_TTL_SECONDS);

  // Day 29: in daily use, and refreshed. The new token ends at the very same moment.
  clock.now += 29 * day;
  assert.equal((await handleSelf(call("/v1/devices", { token }), ENV, account.fetch, clock.now)).status, 200);
  const refreshed = (await body(await handleSelf(call("/v1/token", { token, method: "POST" }), ENV, account.fetch, clock.now))).token as string;
  const claims = await verifyToken(account.key, refreshed, Math.floor(clock.now / 1000));
  assert.deepEqual([claims?.iat, claims?.exp], [paired + 29 * 86400, paired + SELF_DEVICE_TTL_SECONDS]);

  // Day 30: both tokens are over, at the Worker (signature expiry) and at the object (the row is gone).
  clock.now += day;
  for (const lapsed of [token, refreshed]) {
    assert.equal((await handleSelf(call("/v1/devices", { token: lapsed }), ENV, account.fetch, clock.now)).status, 401);
    assert.equal((await handleSelf(call("/v1/token", { token: lapsed, method: "POST" }), ENV, account.fetch, clock.now)).status, 401);
  }
  // The next request that reaches the object drops the lapsed row (and, in the real one, closes its socket).
  await handleSelf(call("/v1/self/pairings", { secret: HOST_SECRET, method: "DELETE" }), ENV, account.fetch, clock.now);
  assert.equal(account.devices.has(PHONE_ID), false);

  // Pairing again with a new code is all it takes, and starts a fresh 30 days.
  const again = "C".repeat(43);
  await offer(again);
  const fresh = (await body(await handleSelf(call("/v1/self/pair", { body: { code: again, device: phone } }), ENV, account.fetch, clock.now))).token as string;
  assert.equal((await verifyToken(account.key, fresh, Math.floor(clock.now / 1000)))?.exp, Math.floor(clock.now / 1000) + SELF_DEVICE_TTL_SECONDS);
  assert.equal((await handleSelf(call("/v1/devices", { token: fresh }), ENV, account.fetch, clock.now)).status, 200);

  // The rules on their own. The desktop's row has no fixed end.
  assert.equal(deviceExpiry(1000), 1000 + SELF_DEVICE_TTL_SECONDS);
  assert.deepEqual([credentialExpired({ expires_at: 0 }, 9e9), credentialExpired({ expires_at: 500 }, 499), credentialExpired({ expires_at: 500 }, 500)], [false, false, true]);
  assert.deepEqual([tokenLifetime(0, 100), tokenLifetime(undefined, 100), tokenLifetime(700, 100), tokenLifetime(100, 100)], [TOKEN_TTL_SECONDS, TOKEN_TTL_SECONDS, 600, 1]);
});

test("the signing key is the deployment's own: made once, cached per isolate, kept through a wipe", async () => {
  assert.ok(isSigningKey(newSigningKey()));
  assert.notEqual(newSigningKey(), newSigningKey());
  assert.equal(isSigningKey(HOST_SECRET), false);
  let asked = 0;
  const account: AccountFetch = async () => { asked++; return new Response(JSON.stringify({ key: "ab".repeat(32) })); };
  const cache: KeyCache = {};
  assert.deepEqual([await signingKey(account, cache), await signingKey(account, cache), asked], ["ab".repeat(32), "ab".repeat(32), 1]);
  // A failed or malformed read is not remembered, and the route answers a generic 500.
  const broken: KeyCache = {};
  await assert.rejects(signingKey(async () => new Response(JSON.stringify({ key: "short" })), broken));
  assert.equal(broken.key, undefined);
  const res = await route(call("/v1/self/probe", { secret: HOST_SECRET }), ENV, async () => new Response("{}", { status: 500 }), 0, {});
  assert.deepEqual([res.status, await body(res)], [500, { error: "server error" }]);

  // Deleting the deployment's state closes every socket and drops every row, but not the key.
  const stored = new Map<string, unknown>([[SIGNING_KEY, "cd".repeat(32)], ["devices", 1]]);
  const closed: string[] = [];
  await deleteSelfState(
    { getWebSockets: () => ["a", "b"], storage: { deleteAll: async () => stored.clear(), get: async (k) => stored.get(k), put: async (k, v) => void stored.set(k, v) } },
    (socket, code, reason) => closed.push(`${socket}:${code}:${reason}`),
  );
  assert.deepEqual([closed, [...stored]], [["a:4001:signed out", "b:4001:signed out"], [[SIGNING_KEY, "cd".repeat(32)]]]);
});
