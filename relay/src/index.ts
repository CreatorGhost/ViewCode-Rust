import { Account } from "./account.ts";
import { readJsonCapped } from "./body.ts";
import { configured } from "./config.ts";
import { type Env, fail, json } from "./env.ts";
import { githubUser } from "./github.ts";
import { MAX_FRAME_BYTES } from "./limits.ts";
import { isKnownRoute } from "./routes.ts";
import { forwardAs, handleSelf, relayMode, SELF_ACCOUNT } from "./self.ts";
import { signToken, verifyToken } from "./token.ts";
import { parseDevice } from "./validate.ts";

export { Account };

const stub = (env: Env, acct: string) => env.ACCOUNT.get(env.ACCOUNT.idFromName(acct));

async function authGithub(request: Request, env: Env): Promise<Response> {
  const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
  if (!(await env.AUTH_LIMITER.limit({ key: ip })).success) return fail(429, "too many sign-in attempts");
  if (!(await env.GLOBAL_AUTH_LIMITER.limit({ key: "all" })).success) return fail(429, "too many sign-in attempts");
  const read = await readJsonCapped(request, MAX_FRAME_BYTES);
  if (read === "too-large") return fail(413, "request too large");
  const body = read === "invalid" ? null : (read.value as { githubToken?: unknown; device?: unknown } | null);
  if (!body || typeof body.githubToken !== "string" || !body.githubToken || body.githubToken.length > 512) {
    return fail(400, "githubToken is required");
  }
  const device = parseDevice(body.device);
  if (typeof device === "string") return fail(400, device);
  const user = await githubUser(body.githubToken, env.GITHUB_CLIENT_ID, env.GITHUB_CLIENT_SECRET);
  if ("status" in user) return fail(user.status, user.error);
  const res = await stub(env, user.id).fetch("https://account/internal/register", { method: "POST", body: JSON.stringify(device) });
  if (!res.ok) return res;
  const { gen } = (await res.json().catch(() => ({}))) as { gen?: unknown };
  const token = await signToken(env.SESSION_SECRET, user.id, device.id, Math.floor(Date.now() / 1000), typeof gen === "number" ? gen : undefined);
  return json({ token, account: user });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Self-hosted (Quick connect): one deployment, one account, the host secret instead of GitHub.
    if (relayMode(env) === "self") return handleSelf(request, env, (forwarded) => stub(env, SELF_ACCOUNT).fetch(forwarded));
    const url = new URL(request.url);
    const path = url.pathname;
    if (!configured(env, path === "/v1/auth/github")) return fail(500, "server error");
    if (path === "/v1/auth/github") return request.method === "POST" ? authGithub(request, env) : fail(405, "method not allowed");

    if (!isKnownRoute(path)) return fail(404, "not found");
    const bearer = /^Bearer (\S+)$/.exec(request.headers.get("authorization") ?? "")?.[1];
    const claims = bearer ? await verifyToken(env.SESSION_SECRET, bearer, Math.floor(Date.now() / 1000)) : null;
    if (!claims) return fail(401, "invalid or expired token");

    if (!(await env.ACCOUNT_LIMITER.limit({ key: claims.acct })).success) return fail(429, "too many requests");

    const res = await stub(env, claims.acct).fetch(forwardAs(request, claims));
    if (path !== "/v1/token" || !res.ok) return res;
    const { gen } = (await res.json().catch(() => ({}))) as { gen?: unknown };
    return json({ token: await signToken(env.SESSION_SECRET, claims.acct, claims.dev, Math.floor(Date.now() / 1000), typeof gen === "number" ? gen : undefined) });
  },
} satisfies ExportedHandler<Env>;
