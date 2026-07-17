// supabase/functions/checkout-return/index.ts
//
// Static landing page shown after a Stripe Checkout redirect (success or
// cancel) — see create-checkout/index.ts's success_url / cancel_url. Reads
// only the `status` query param and returns inline-styled HTML; there is no
// entitlement logic here (the webhook, S4, grants the entitlement
// out-of-band) and nothing here needs to be trusted, since the query param
// only selects between two fixed, non-user-supplied strings — it is never
// reflected into the HTML itself.
//
// Task S3: code + commit only, no deployment. Deployed at S6 with
// verify_jwt: OFF — by design this function reads zero secrets, zero env
// vars, and performs zero auth: it's a public, stateless redirect target.

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Builds the fixed-shape HTML page. `heading` and `message` are always one
 * of the two literal strings chosen in the handler below — never derived
 * from request input — so there is nothing here to escape or sanitize.
 */
function htmlResponse(heading: string, message: string): Response {
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Cleophis</title>
</head>
<body style="margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0E1218;color:#E6E9EF;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;">
<div style="text-align:center;max-width:28rem;padding:2rem;">
<h1 style="color:#35D0BA;font-size:1.5rem;margin:0 0 0.75rem;">${heading}</h1>
<p style="margin:0;line-height:1.5;">${message}</p>
</div>
</body>
</html>
`;
  return new Response(html, {
    status: 200,
    headers: { "Content-Type": "text/html; charset=utf-8" },
  });
}

Deno.serve((req: Request) => {
  if (req.method !== "GET") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  const url = new URL(req.url);
  const status = url.searchParams.get("status");

  // Boolean branch only — the raw query param value is never echoed into
  // the response, per brief ("no user data echoed").
  if (status === "success") {
    return htmlResponse(
      "Payment complete",
      "You can close this tab and return to the Cleophis app — your download starts automatically.",
    );
  }
  return htmlResponse(
    "Payment cancelled",
    "No charge was made. You can close this tab and return to the Cleophis app.",
  );
});
