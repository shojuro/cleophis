# Verification — Cloudflare domain milestone (cleophis.com)

Branch `feat/cloudflare-domain` (PR #6). One code commit (`e1f5306`) — by
design: both cutover seams were built in earlier milestones, so this
milestone is DNS + one secret + URL constant swaps + docs.

## Infrastructure changes (Cloudflare zone `cleophis.com`, Free plan)

All applied via the API with the scoped token (`CLOUDFLARE_API_TOKEN` in
`~/.env`; Zone:Read, DNS:Edit, Zone WAF:Edit, Zone Settings:Edit —
restricted to this zone), 2026-07-18:

| Change | Value | Proof |
| --- | --- | --- |
| Zone active | `cleophis.com` (id `aa8150fa…`) | API `status: active` |
| `dl` CNAME | → `f005.backblazeb2.com`, **proxied** | created OK |
| `pay` CNAME | → `shojuro.github.io`, DNS-only | created OK |
| SSL mode | Full (strict) | setting OK |
| Always Use HTTPS | on | setting OK |
| WAF custom rule | skip `bic/securityLevel/uaBlock/zoneLockdown/waf/rateLimit` for `http.host eq "dl.cleophis.com"` | ruleset PUT, 1 rule |
| GitHub Pages | `cname: pay.cleophis.com`, `https_enforced: true`; `CNAME` file on gh-pages | Pages API |
| Supabase secret | `B2_DOWNLOAD_BASE_URL=https://dl.cleophis.com` | secrets POST 201 |
| Stripe portal config | `bpc_1TuK9N…` `default_return_url` → `https://pay.cleophis.com/pay/portal-return.html` | API update echoed |

## Live proofs (all 2026-07-18)

- **Download path**: proxied request to the GGUF path answers `401` via
  `server: cloudflare` (Cloudflare→B2 chain up, auth enforced). Probe
  account (real JWT, temp subscription row): `download-url` minted
  `url` host **`dl.cleophis.com`**; ranged GET bytes 0–1048575 through
  the proxy → **`206`, exactly 1 MiB, `GGUF` magic bytes**. Probe deleted
  (`remaining: 0`).
- **Return pages**: `https://pay.cleophis.com/pay/success.html` → 200
  (cert issued ~1 min after Pages cname set); HTTPS enforced.
- **Checkout URLs**: post-deploy probe minted a session; Stripe's newest
  checkout session shows `success_url`/`cancel_url` on
  `https://pay.cleophis.com/pay/…`. Probe deleted.
- **Back-compat**: pre-cutover `shojuro.github.io/cleophis/pay/…` URLs
  301-redirect to the custom domain (GitHub Pages behavior), so sessions
  and portal configs minted before the cutover still land.
- **Rollback**: delete the `B2_DOWNLOAD_BASE_URL` secret → URLs fall back
  to B2's native hostname; tokens are hostname-agnostic either way.

## Invariants

- CSP byte-identical (no client code changed at all this milestone — the
  Rust client trusts the URL the trusted edge function returns, verified
  no host pinning exists).
- `download-url`'s entitlement/expiry/source fences untouched; only URL
  construction input (the secret) changed.
- No secret values in the diff; the CF token lives in `~/.env` (0600) and
  was never echoed.

## Pending — user E2E

- [ ] Checkout round-trip from the app lands on `pay.cleophis.com`
      (browser tab after paying/cancelling shows the custom domain).
- [ ] Optional: a full 2 GB model download through `dl.cleophis.com` on
      the target machine (delete the local model file first to force it;
      the ranged-GET proof above covers the path, so this is belt and
      braces, not a gate).
