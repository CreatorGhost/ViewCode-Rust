/**
 * Self-hosted mode ("Quick connect"): the Worker runs on the user's own Cloudflare account
 * and is authenticated by a per-deployment host secret instead of GitHub sign-in. The frame
 * format, the device list and the end-to-end encryption are the shared relay's; only how a
 * device gets its token differs:
 *
 * - The desktop proves the host secret (`Authorization: Bearer <HOST_SECRET>`) and registers
 *   itself as the deployment's admin device (`POST /v1/self/host`). The host secret does
 *   nothing else: device tokens are signed with a key the deployment makes once and keeps
 *   in its Durable Object, so rotating the host secret leaves every paired phone paired.
 * - The desktop offers a one-time pairing (`POST /v1/self/pairings`); a phone redeems it
 *   without any account (`POST /v1/self/pair`) and gets an ordinary relay token.
 *
 * Everything here is pure (no Durable Object, no bindings) so `node --test` covers it. The
 * Durable Object in `account.ts` supplies the storage behind `PairingStore`.
 */
import { readJsonCapped } from "./body.ts";
import { MAX_FRAME_BYTES, SELF_DEVICE_TTL_SECONDS, TOKEN_TTL_SECONDS } from "./limits.ts";
import { isKnownRoute } from "./routes.ts";
import { signToken, verifyToken, type TokenClaims } from "./token.ts";
import { parseDevice } from "./validate.ts";

export interface SelfEnv {
  RELAY_MODE?: string;
  HOST_SECRET?: string;
}

export const SELF_ACCOUNT = "self";
/** Bumped when the self-hosted HTTP contract changes incompatibly; the desktop's probe reads it. */
export const SELF_PROTOCOL = 1;
export const MIN_HOST_SECRET_CHARS = 32;
/** A pairing is valid for at most this long, whatever the desktop asks for. */
export const PAIRING_TTL_MS = 5 * 60 * 1000;
/** Unredeemed pairings kept at once; an older one must be used, cancelled or expire first. */
export const MAX_PENDING_PAIRINGS = 8;
/** Used and expired rows are kept this long, so a replay is still recognised as one. */
const PAIRING_KEEP_MS = 60 * 60 * 1000;

const enc = new TextEncoder();
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const fail = (status: number, error: string) => json({ error }, status);

export const relayMode = (env: SelfEnv): "shared" | "self" => (env.RELAY_MODE === "self" ? "self" : "shared");

/** A self-hosted Worker answers nothing until its host secret is stored (deploy comes first). */
export const selfConfigured = (env: SelfEnv): env is SelfEnv & { HOST_SECRET: string } =>
  typeof env.HOST_SECRET === "string" && env.HOST_SECRET.length >= MIN_HOST_SECRET_CHARS;

function hex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export async function sha256Hex(text: string): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", enc.encode(text)));
}

/** 256 random bits (hex): the deployment's token-signing key, made once by its Durable Object. */
export function newSigningKey(): string {
  return hex(crypto.getRandomValues(new Uint8Array(32)).buffer);
}

export const isSigningKey = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);

/** One per isolate: the key never changes for the life of a deployment, so it is asked for once. */
export interface KeyCache {
  key?: Promise<string>;
}
const isolateKeys: KeyCache = {};

/** The signing key, from the Durable Object. A failed read is not remembered. */
export function signingKey(account: AccountFetch, cache: KeyCache): Promise<string> {
  cache.key ??= account(internal("/internal/self/key", undefined, "GET"))
    .then(async (res) => {
      const { key } = (await res.json().catch(() => ({}))) as { key?: unknown };
      if (!res.ok || !isSigningKey(key)) throw new Error("no signing key");
      return key;
    })
    .catch((error) => {
      cache.key = undefined;
      throw error;
    });
  return cache.key;
}

// ---- How long a paired device stays paired ----

/** When a device that pairs now lapses (unix seconds). The desktop's own row has no fixed end (0). */
export const deviceExpiry = (nowS: number): number => nowS + SELF_DEVICE_TTL_SECONDS;

/** A paired device's credential has lapsed. Rows without a fixed end (the desktop, the shared relay) never do. */
export const credentialExpired = (row: { expires_at: number }, nowS: number): boolean => row.expires_at > 0 && row.expires_at <= nowS;

/** A token's lifetime in seconds: up to the device's fixed end, never past it; the usual 30 days without one. */
export const tokenLifetime = (expiresAt: unknown, nowS: number): number =>
  typeof expiresAt === "number" && expiresAt > 0 ? Math.max(1, expiresAt - nowS) : TOKEN_TTL_SECONDS;

/** Whether the request carries the host secret. Fixed-length digests are compared without an early exit. */
export async function hostAuthorized(hostSecret: string, authorization: string | null): Promise<boolean> {
  const given = /^Bearer (\S+)$/.exec(authorization ?? "")?.[1];
  if (!given || given.length > 512) return false;
  const [a, b] = await Promise.all([hostSecret, given].map(async (v) => new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(v)))));
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

// ---- Pairing ----

export interface PairingRow {
  code_hash: string;
  expires_at: number;
  used_at: number | null;
}

export interface PairingStore {
  get(codeHash: string): PairingRow | undefined;
  insert(row: PairingRow): void;
  markUsed(codeHash: string, at: number): void;
  /** Deletes rows that expired before `before`. */
  prune(before: number): void;
  /** Rows not used and not expired at `now`. */
  pending(now: number): number;
  /** Deletes every unused row. */
  cancel(): void;
}

export type PairingVerdict = "ok" | "unknown" | "expired" | "used";

export const isCodeHash = (v: unknown): v is string => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);
/** What the phone sends: base64url of a SHA-256 digest (see docs/remote-access.md, Quick connect). */
export const isPairingCode = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_-]{43}$/.test(v);

/** `{ codeHash, ttlSeconds }` from the desktop. The relay's own clock sets the expiry. */
export function parsePairingOffer(v: unknown, nowMs: number): PairingRow | string {
  if (typeof v !== "object" || v === null) return "body must be an object";
  const o = v as Record<string, unknown>;
  if (!isCodeHash(o.codeHash)) return "codeHash must be a SHA-256 hex digest";
  const ttl = o.ttlSeconds;
  if (typeof ttl !== "number" || !Number.isInteger(ttl) || ttl < 1 || ttl * 1000 > PAIRING_TTL_MS) {
    return `ttlSeconds must be 1-${PAIRING_TTL_MS / 1000}`;
  }
  return { code_hash: o.codeHash, expires_at: nowMs + ttl * 1000, used_at: null };
}

/** Records an offered pairing. Returns an error message, or null when stored. */
export function offerPairing(store: PairingStore, row: PairingRow, nowMs: number): string | null {
  store.prune(nowMs - PAIRING_KEEP_MS);
  // A hash is offered once: re-offering it could revive a used or expired code.
  if (store.get(row.code_hash)) return "pairing already offered";
  if (store.pending(nowMs) >= MAX_PENDING_PAIRINGS) return `too many pairings are waiting (${MAX_PENDING_PAIRINGS})`;
  store.insert(row);
  return null;
}

export function pairingVerdict(row: PairingRow | undefined, nowMs: number): PairingVerdict {
  if (!row) return "unknown";
  if (row.used_at !== null) return "used";
  return row.expires_at > nowMs ? "ok" : "expired";
}

/** Single use: an `ok` verdict marks the row used before returning. */
export function redeemPairing(store: PairingStore, codeHash: string, nowMs: number): PairingVerdict {
  const verdict = pairingVerdict(store.get(codeHash), nowMs);
  if (verdict === "ok") store.markUsed(codeHash, nowMs);
  return verdict;
}

// ---- Who may do what on a self-hosted deployment ----

/** Only the desktop (admin) removes other devices; any device may remove itself. */
export const mayRevoke = (mode: "shared" | "self", caller: { id: string; admin: number }, target: string): boolean =>
  mode === "shared" || caller.admin === 1 || caller.id === target;

/** Only the desktop hosts or deletes the deployment's state; a paired phone never does. */
export const mayAdminister = (mode: "shared" | "self", caller: { admin: number }): boolean => mode === "shared" || caller.admin === 1;

/** A pairing never takes over the desktop's own device id, even with its (public) key. */
export const mayPairAs = (existing: { admin: number } | undefined): boolean => existing?.admin !== 1;

// ---- The Worker's routes in self-hosted mode ----

/** Reaches the deployment's one Durable Object. */
export type AccountFetch = (request: Request) => Promise<Response>;

const internal = (path: string, body?: unknown, method = "POST") =>
  new Request(`https://account${path}`, { method, body: body === undefined ? undefined : JSON.stringify(body) });

export const selfAccountView = { id: SELF_ACCOUNT, login: "", avatarUrl: "" };

async function capped(request: Request): Promise<Record<string, unknown> | Response> {
  const read = await readJsonCapped(request, MAX_FRAME_BYTES);
  if (read === "too-large") return fail(413, "request too large");
  if (read === "invalid" || typeof read.value !== "object" || read.value === null) return fail(400, "body must be a JSON object");
  return read.value as Record<string, unknown>;
}

/** A token for `device` after the Durable Object accepted it (its reply carries the row's `gen`). */
async function sign(secret: string, res: Response, device: string, nowS: number): Promise<string | Response> {
  if (!res.ok) return res;
  const { gen, expiresAt } = (await res.json().catch(() => ({}))) as { gen?: unknown; expiresAt?: unknown };
  return signToken(secret, SELF_ACCOUNT, device, nowS, typeof gen === "number" ? gen : undefined, tokenLifetime(expiresAt, nowS));
}

async function issue(secret: string, res: Response, device: string, nowS: number): Promise<Response> {
  const token = await sign(secret, res, device, nowS);
  return typeof token === "string" ? json({ token, account: selfAccountView }) : token;
}

/** Adds what the Durable Object trusts (set only here, after the token check) and forwards. */
export function forwardAs(request: Request, claims: TokenClaims): Request {
  const headers = new Headers(request.headers);
  headers.set("x-device-id", claims.dev);
  headers.set("x-token-iat", String(claims.iat));
  headers.delete("x-token-gen");
  if (claims.gen !== undefined) headers.set("x-token-gen", String(claims.gen));
  headers.delete("authorization");
  return new Request(request, { headers });
}

export async function handleSelf(request: Request, env: SelfEnv, account: AccountFetch, nowMs: number = Date.now(), keys: KeyCache = isolateKeys): Promise<Response> {
  // Deployed but the secret is not stored yet (or is too short): say nothing useful.
  if (!selfConfigured(env)) return fail(500, "server error");
  const path = new URL(request.url).pathname;
  const nowS = Math.floor(nowMs / 1000);
  let secret: string;
  try {
    secret = await signingKey(account, keys);
  } catch {
    return fail(500, "server error");
  }

  if (path === "/v1/self/pair") {
    if (request.method !== "POST") return fail(405, "method not allowed");
    const body = await capped(request);
    if (body instanceof Response) return body;
    // One answer for every bad code, so a guess learns nothing about which codes exist.
    if (!isPairingCode(body.code)) return fail(401, "pairing code is not valid or has expired");
    const device = parseDevice(body.device);
    if (typeof device === "string") return fail(400, device);
    const res = await account(internal("/internal/self/pair", { codeHash: await sha256Hex(body.code), device }));
    return issue(secret, res, device.id, nowS);
  }

  if (path.startsWith("/v1/self/")) {
    if (!(await hostAuthorized(env.HOST_SECRET, request.headers.get("authorization")))) return fail(401, "invalid host secret");
    if (path === "/v1/self/probe") {
      return request.method === "GET" ? json({ ok: true, mode: "self", protocol: SELF_PROTOCOL }) : fail(405, "method not allowed");
    }
    if (path === "/v1/self/host") {
      if (request.method !== "POST") return fail(405, "method not allowed");
      const body = await capped(request);
      if (body instanceof Response) return body;
      const device = parseDevice(body.device);
      if (typeof device === "string") return fail(400, device);
      return issue(secret, await account(internal("/internal/self/host", device)), device.id, nowS);
    }
    if (path === "/v1/self/pairings") {
      if (request.method === "DELETE") return account(internal("/internal/self/pairings", undefined, "DELETE"));
      if (request.method !== "POST") return fail(405, "method not allowed");
      const body = await capped(request);
      if (body instanceof Response) return body;
      const offer = parsePairingOffer(body, nowMs);
      if (typeof offer === "string") return fail(400, offer);
      return account(internal("/internal/self/pairings", offer));
    }
    return fail(404, "not found");
  }

  // Everything else is the shared relay's authenticated API, with tokens signed for this deployment.
  if (!isKnownRoute(path)) return fail(404, "not found");
  const bearer = /^Bearer (\S+)$/.exec(request.headers.get("authorization") ?? "")?.[1];
  const claims = bearer ? await verifyToken(secret, bearer, nowS) : null;
  if (!claims || claims.acct !== SELF_ACCOUNT) return fail(401, "invalid or expired token");
  const res = await account(forwardAs(request, claims));
  if (path !== "/v1/token") return res;
  const token = await sign(secret, res, claims.dev, nowS);
  return typeof token === "string" ? json({ token }) : token;
}
