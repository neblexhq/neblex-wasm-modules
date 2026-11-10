# jwt-decoder

Decodes JSON Web Tokens (JWT), such as OAuth access tokens, OpenID Connect ID tokens and webhook tokens, into their header and claims, with readable dates and an expiry check. Use it to route or log by user, tenant or scope, to spot tokens that are about to expire, or to debug an authentication step.

**This module does not verify signatures.** `signature_verified` is always `false`. Decoding shows what a token claims, not that the claims are true: never grant access based on this output alone. Verify tokens with the issuer's keys in the system that accepts them.

## Input

One record or an array of records, each with a token. A `Bearer ` prefix, as copied from an `Authorization` header, is fine:

```json
{ "request_id": "r-1", "token": "Bearer eyJhbGciOiJSUzI1NiIs...", "_options": { "now": "2025-11-09T17:30:00Z" } }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the token | `token` |
| `now` | The current time, as ISO 8601 or unix seconds. Modules have no clock, so pass the flow's run time here to get `expired` and `valid_now` | none |
| `leeway_s` | Seconds of clock difference to allow when checking `exp` and `nbf` | `0` |
| `keep_source` | Keep the token in the output | `false` |

## Output

```json
{
  "ok": true,
  "request_id": "r-1",
  "header": { "alg": "RS256", "typ": "JWT", "kid": "key-2026-11" },
  "claims": {
    "iss": "https://login.example.com/", "sub": "user-4471", "aud": "api://orders",
    "iat": 1762707600, "exp": 1762711200, "scope": "orders.read", "email": "jane@example.com"
  },
  "algorithm": "RS256",
  "key_id": "key-2026-11",
  "issuer": "https://login.example.com/",
  "subject": "user-4471",
  "audience": ["api://orders"],
  "issued_at": "2025-11-09T17:00:00Z",
  "not_before": null,
  "expires_at": "2025-11-09T18:00:00Z",
  "lifetime_s": 3600,
  "expired": false,
  "not_yet_valid": false,
  "valid_now": true,
  "expires_in_s": 1800,
  "signature_verified": false,
  "warnings": []
}
```

- Dates are UTC. `audience` is always a list.
- Without `now`, `expired` and `valid_now` are `null`.
- `warnings` flags unsigned tokens (`alg: none` or an empty signature) and tokens with no expiry.
- The token is removed from the output by default, so it doesn't end up in run history or logs.
- Encrypted tokens (JWE, five parts) return `ok: false`: their claims can't be read without the key.

## Use it in a Neblex flow

1. Download `jwt-decoder.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `jwt-decoder`.
3. Add a step that uses the module's **Run module** operation after a webhook or API step that returns a token or an Authorization header. Map it to `token`, and pass the run time as `_options.now` for expiry checks.

## Build and test

```sh
../build.sh jwt-decoder
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
