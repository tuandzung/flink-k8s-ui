# Local Development

## Run in fixture mode

```bash
npm run dev
```

Open `http://localhost:3000`.

## Runtime and auth model

- `npm start` delegates to `npm run start:rust`, which runs the supported Rust backend in `apps/api-rs`.
- `npm run dev` keeps fixture-mode local development intentionally unauthenticated.
- Live-mode traffic uses app-owned OIDC login plus a same-origin session; the signed-out shell remains public while `/api/*` stays session-gated.
- Set a canonical external base URL and callback path so `/auth/login` and `/auth/callback` stay correct behind the public host.
- `/metrics` is an operations endpoint and should stay on a separate internal-only service or scrape path.

## Run tests

```bash
cargo test --manifest-path apps/api-rs/Cargo.toml
npm test
```

- `cargo test --manifest-path apps/api-rs/Cargo.toml` runs backend tests.
- `npm test` runs frontend render tests.

## Build

```bash
npm run build
```

This produces a Rust release binary plus the static web assets under `dist/`.

## Build the container image

```bash
docker build -f deploy/api/Dockerfile -t flink-job-ui:latest .
```

## Full smoke workflow

```bash
npm run ci:smoke
```

This runs:

- `cargo test`
- `npm test`
- Docker image build
- a fixture-mode container smoke check

## Run against Kubernetes

The example deployment assumes the Rust backend is the only supported production runtime and that the app owns OIDC/session auth itself.

Provide either:

1. `FLINK_UI_CLUSTERS_JSON` with one or more cluster objects, or
2. in-cluster variables such as `KUBERNETES_SERVICE_HOST` plus mounted service account token

If your local-development Kubernetes API is exposed on `localhost`, `*.localhost`, or another loopback address and uses a self-signed certificate, you may set `K8S_INSECURE_SKIP_TLS_VERIFY=true` (or JSON `insecureSkipTlsVerify: true`) as an explicit local-only escape hatch. The Rust runtime rejects that bypass for non-loopback / production-style cluster URLs.

Example:

```bash
export FIXTURE_MODE=false
export OIDC_ISSUER_URL='https://accounts.example.com/realms/platform'
export OIDC_CLIENT_ID='flink-job-ui'
export OIDC_CLIENT_SECRET='replace-me'
export OIDC_EXTERNAL_BASE_URL='https://flink-jobs.example.com'
export OIDC_CALLBACK_PATH='/auth/callback'
export OIDC_SCOPES='openid profile email'
export OIDC_REQUEST_TIMEOUT_MS='15000'
export SESSION_COOKIE_SECRET='replace-with-32-byte-secret'
export SESSION_SECURE_COOKIE=true
export FLINK_UI_CLUSTERS_JSON='[
  {
    "name": "prod",
    "apiUrl": "https://kubernetes.default.svc",
    "bearerTokenFile": "/var/run/secrets/kubernetes.io/serviceaccount/token",
    "caCertFile": "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt",
    "namespaces": ["analytics", "payments"],
    "flinkApiVersion": "v1beta1"
  }
]'
npm start
```

If you test the live OIDC flow through `kubectl port-forward` on `http://localhost:3000`,
set `SESSION_SECURE_COOKIE=false` for that local HTTP session. Keep it `true` for real
HTTPS ingress traffic.

If your cluster has slow outbound DNS/TLS handshakes to Google, raise
`OIDC_REQUEST_TIMEOUT_MS` before increasing the broader `REQUEST_TIMEOUT_MS` used for
Kubernetes/Flink upstream calls.

## Notes

- The default `npm start` / `npm run dev` path now runs the Rust backend in `apps/api-rs`.
- `FlinkSessionJob` collection is best-effort; clusters without that CR kind still work.
- Flink REST enrichment is optional and never blocks job listing.
- There is no separate Node backend runtime path anymore.
- Do not expose `/metrics` on the public ingress; keep it on an internal-only operations path.
