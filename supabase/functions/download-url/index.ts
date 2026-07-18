// supabase/functions/download-url/index.ts
//
// Mints a short-lived Backblaze B2 download authorization for an entitled
// user's model file. Called by the Rust client's `mint_download_url`
// (src-tauri/src/cloud/rest.rs) — POST { model_id } with a Supabase user
// JWT in the Authorization header.
//
// Task C6a: code + commit only, no deployment. Deployment + secrets happen
// in C6b via tools/deploy-download-url.sh once the user's B2 keys arrive.
//
// Import: pinned to jsr:@supabase/supabase-js@2 — this is the import form
// the Supabase docs currently recommend for Edge Functions (superseding
// the older esm.sh/deno.land/x forms seen in some legacy examples).
import { createClient } from "jsr:@supabase/supabase-js@2.110.7";

// In-source model registry. Keep in sync with the catalog on the client
// side (src/app.js / src-tauri) — this function is the source of truth for
// what bytes actually exist in the B2 bucket. allowedSources fences which
// entitlements.source values grant access to each model: paid models list
// ["purchase"] (Stripe-gated, granted by stripe-webhook — see S4/create-checkout);
// free models would list ["library"]. The fence is per-model, checked below
// via .in("source", model.allowedSources) on the entitlement query.
const MODELS: Record<string, { prefix: string; file: string; bytes: number; allowedSources: string[] }> = {
  "socratic-tutor": {
    prefix: "models/socratic-tutor/1/",
    file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
    bytes: 2019377696,
    allowedSources: ["purchase"],
  },
};

const B2_AUTHORIZE_ACCOUNT_URL = "https://api.backblazeb2.com/b2api/v2/b2_authorize_account";
const DOWNLOAD_AUTH_VALID_SECONDS = 21600; // 6h — matches b2_get_download_authorization call.
const B2_AUTH_CACHE_MS = 20 * 60 * 60 * 1000; // 20h — brief says "reuse if < 20h old".

interface B2Auth {
  apiUrl: string;
  downloadUrl: string;
  authorizationToken: string;
  expiresAtMs: number;
}

// Module-level cache reused across warm invocations of this isolate. Reset
// to null whenever a request hits a B2 401 with the cached token, forcing
// one re-authorization + retry (see handling around b2_get_download_authorization
// below).
let b2Auth: B2Auth | null = null;

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Returns the env value, or null if unset/empty. Never logs the value. */
function readEnv(name: string): string | null {
  const value = Deno.env.get(name);
  return value && value.length > 0 ? value : null;
}

/**
 * Calls B2's b2_authorize_account. Returns null (and logs status/body,
 * server-side only) on any non-2xx or network failure — callers turn that
 * into a 502 storage_unavailable.
 */
async function authorizeB2(keyId: string, appKey: string): Promise<B2Auth | null> {
  let res: Response;
  try {
    res = await fetch(B2_AUTHORIZE_ACCOUNT_URL, {
      headers: { Authorization: `Basic ${btoa(`${keyId}:${appKey}`)}` },
    });
  } catch (err) {
    console.error(`B2 b2_authorize_account request failed: ${String(err)}`);
    return null;
  }
  if (!res.ok) {
    const body = await res.text().catch(() => "<unreadable>");
    console.error(`B2 b2_authorize_account failed: status=${res.status} body=${body}`);
    return null;
  }
  let data: { apiUrl?: unknown; downloadUrl?: unknown; authorizationToken?: unknown };
  try {
    data = await res.json();
  } catch {
    console.error("B2 b2_authorize_account returned malformed JSON");
    return null;
  }
  if (
    typeof data.apiUrl !== "string" ||
    typeof data.downloadUrl !== "string" ||
    typeof data.authorizationToken !== "string"
  ) {
    console.error("B2 b2_authorize_account response missing expected fields");
    return null;
  }
  return {
    apiUrl: data.apiUrl,
    downloadUrl: data.downloadUrl,
    authorizationToken: data.authorizationToken,
    expiresAtMs: Date.now() + B2_AUTH_CACHE_MS,
  };
}

/** Returns the cached B2 account auth if fresh (< 20h old), else re-authorizes. */
async function getB2Auth(keyId: string, appKey: string): Promise<B2Auth | null> {
  if (b2Auth && b2Auth.expiresAtMs > Date.now()) {
    return b2Auth;
  }
  const fresh = await authorizeB2(keyId, appKey);
  b2Auth = fresh;
  return fresh;
}

Deno.serve(async (req: Request) => {
  // Assumption: the brief specifies a body only for the 400 case; 405 gets
  // its own distinct error code here (still JSON, per "all responses
  // Content-Type: application/json").
  if (req.method !== "POST") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  let payload: unknown;
  try {
    payload = await req.json();
  } catch {
    return jsonResponse({ error: "bad_request" }, 400);
  }
  if (typeof payload !== "object" || payload === null) {
    return jsonResponse({ error: "bad_request" }, 400);
  }
  const modelId = (payload as Record<string, unknown>).model_id;
  if (typeof modelId !== "string" || modelId.length === 0) {
    return jsonResponse({ error: "bad_request" }, 400);
  }

  // Env check happens right after basic request validation, before any
  // network calls, so a misconfigured deployment fails fast and
  // deterministically regardless of which downstream step would have
  // needed the missing var. Not explicitly ordered in the brief; documented
  // as an assumption.
  const supabaseUrl = readEnv("SUPABASE_URL");
  const serviceRoleKey = readEnv("SUPABASE_SERVICE_ROLE_KEY");
  const b2KeyId = readEnv("B2_KEY_ID");
  const b2AppKey = readEnv("B2_APP_KEY");
  const b2BucketId = readEnv("B2_BUCKET_ID");
  const b2BucketName = readEnv("B2_BUCKET_NAME");
  // B2_DOWNLOAD_BASE_URL is optional — read again later, no upfront check.

  const missing: string[] = [];
  if (!supabaseUrl) missing.push("SUPABASE_URL");
  if (!serviceRoleKey) missing.push("SUPABASE_SERVICE_ROLE_KEY");
  if (!b2KeyId) missing.push("B2_KEY_ID");
  if (!b2AppKey) missing.push("B2_APP_KEY");
  if (!b2BucketId) missing.push("B2_BUCKET_ID");
  if (!b2BucketName) missing.push("B2_BUCKET_NAME");
  if (missing.length > 0) {
    // Names only — never values — per brief.
    console.error(`download-url misconfigured: missing env ${missing.join(", ")}`);
    return jsonResponse({ error: "misconfigured" }, 500);
  }

  const authHeader = req.headers.get("Authorization") ?? "";
  const jwt = authHeader.startsWith("Bearer ") ? authHeader.slice("Bearer ".length).trim() : "";
  if (!jwt) {
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  const supabase = createClient(supabaseUrl!, serviceRoleKey!);

  let userId: string;
  try {
    const { data: userData, error: userErr } = await supabase.auth.getUser(jwt);
    if (userErr || !userData?.user) {
      return jsonResponse({ error: "unauthorized" }, 401);
    }
    userId = userData.user.id;
  } catch (err) {
    // Never log the jwt itself.
    console.error(`auth.getUser failed: ${String(err)}`);
    return jsonResponse({ error: "unauthorized" }, 401);
  }

  // Object.hasOwn fences the lookup against prototype-chain keys
  // ("__proto__", "constructor", "toString", ...) that would otherwise
  // return a truthy value from MODELS[modelId] and bypass this 404 gate —
  // the paywall must not depend on B2 rejecting a bogus prefix downstream.
  if (!Object.hasOwn(MODELS, modelId)) {
    return jsonResponse({ error: "unknown_model" }, 404);
  }
  const model = MODELS[modelId];

  // Entitlement check: a matching row must exist with expires_at either
  // null or in the future.
  const nowIso = new Date().toISOString();
  const { data: entRows, error: entErr } = await supabase
    .from("entitlements")
    .select("id")
    .eq("user_id", userId)
    .eq("model_id", modelId)
    .in("source", model.allowedSources)
    .or("expires_at.is.null,expires_at.gt." + nowIso);

  if (entErr) {
    // Not one of the brief's named error cases (DB/network failure on the
    // entitlement query) — assumption: treat as a generic internal error,
    // distinct from "misconfigured" (which is env-only) and from B2's
    // "storage_unavailable" (which is B2-specific).
    console.error(`entitlements query failed: ${entErr.message}`);
    return jsonResponse({ error: "internal_error" }, 500);
  }
  if (!entRows || entRows.length === 0) {
    return jsonResponse({ error: "not_entitled", message: "You don't own this model." }, 403);
  }

  // --- B2 flow ---
  let auth = await getB2Auth(b2KeyId!, b2AppKey!);
  if (!auth) {
    return jsonResponse({ error: "storage_unavailable" }, 502);
  }

  const requestDownloadAuth = (a: B2Auth) =>
    fetch(`${a.apiUrl}/b2api/v2/b2_get_download_authorization`, {
      method: "POST",
      headers: {
        Authorization: a.authorizationToken,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        bucketId: b2BucketId,
        fileNamePrefix: model.prefix,
        validDurationInSeconds: DOWNLOAD_AUTH_VALID_SECONDS,
      }),
    });

  let downloadAuthRes: Response;
  try {
    downloadAuthRes = await requestDownloadAuth(auth);
  } catch (err) {
    console.error(`B2 b2_get_download_authorization request failed: ${String(err)}`);
    return jsonResponse({ error: "storage_unavailable" }, 502);
  }

  if (downloadAuthRes.status === 401) {
    // Cached token stale/invalid — clear cache and re-auth once.
    b2Auth = null;
    auth = await getB2Auth(b2KeyId!, b2AppKey!);
    if (!auth) {
      return jsonResponse({ error: "storage_unavailable" }, 502);
    }
    try {
      downloadAuthRes = await requestDownloadAuth(auth);
    } catch (err) {
      console.error(`B2 b2_get_download_authorization retry failed: ${String(err)}`);
      return jsonResponse({ error: "storage_unavailable" }, 502);
    }
  }

  if (!downloadAuthRes.ok) {
    const body = await downloadAuthRes.text().catch(() => "<unreadable>");
    console.error(
      `B2 b2_get_download_authorization failed: status=${downloadAuthRes.status} body=${body}`,
    );
    return jsonResponse({ error: "storage_unavailable" }, 502);
  }

  let downloadAuthData: { authorizationToken?: unknown };
  try {
    downloadAuthData = await downloadAuthRes.json();
  } catch {
    console.error("B2 b2_get_download_authorization returned malformed JSON");
    return jsonResponse({ error: "storage_unavailable" }, 502);
  }
  const authorizationToken = downloadAuthData.authorizationToken;
  if (typeof authorizationToken !== "string" || authorizationToken.length === 0) {
    console.error("B2 b2_get_download_authorization response missing authorizationToken");
    return jsonResponse({ error: "storage_unavailable" }, 502);
  }

  const base = (readEnv("B2_DOWNLOAD_BASE_URL") || auth.downloadUrl).replace(/\/+$/, "");
  const url = `${base}/file/${b2BucketName}/${model.prefix}${model.file}`;
  const expiresAt = new Date(Date.now() + DOWNLOAD_AUTH_VALID_SECONDS * 1000).toISOString();

  return jsonResponse(
    {
      url,
      authorization: authorizationToken,
      expiresAt,
      fileBytes: model.bytes,
    },
    200,
  );
});
