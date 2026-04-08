# Flink K8s UI

Read-only web UI for viewing Apache Flink jobs running on Kubernetes.

This project serves a lightweight dashboard that aggregates `FlinkDeployment` and `FlinkSessionJob` resources, normalizes their status into a small UI-friendly contract, and optionally enriches jobs from the Flink REST API.

## What this project does

- lists Flink jobs across one or more Kubernetes namespaces
- supports both `FlinkDeployment` and `FlinkSessionJob`
- shows normalized status, health, warnings, and job metadata in a simple browser UI
- optionally enriches jobs with Flink REST overview data
- serves static UI assets and API routes from a single Rust service
- supports fixture mode for local development without a cluster

## Current runtime model

- **Supported backend runtime:** Rust (`apps/api-rs`)
- **UI:** static assets under `apps/web/public`
- **Default local entrypoint:** `npm run dev`
- **Default production-style entrypoint:** `npm start`
- **Production auth model:** the Rust service owns OIDC login and same-origin session auth
- **Protected production routes:** `/api/*`
- **Public auth/session routes:** `/auth/login`, `/auth/callback`, `/auth/logout`, `/api/session`, and the signed-out shell at `/`
- **Operational metrics route:** `/metrics` should stay on an internal-only service path
- **Always-open operational routes:** `/healthz`, `/readyz`

There is **no separate Node backend runtime path** anymore. Node is used for the frontend test harness and helper scripts.

## Features

- cluster + namespace + status filtering
- free-text search across resource name and job name
- summary cards for normalized Flink job states
- warning surfacing for partial enrichment or upstream issues
- details drawer with sanitized status details
- metrics snapshot endpoint for request/error counts
- Docker build path and Kubernetes deployment manifests

## Repository layout

```text
.
├── apps/
│   ├── api-rs/          # Rust HTTP server, config, adapters, domain logic
│   └── web/public/      # Static frontend assets
├── deploy/api/          # Dockerfile + Kubernetes manifests
├── docs/                # Architecture and local development docs
├── fixtures/            # Fixture-mode sample payloads
├── scripts/             # Build and smoke-test scripts
└── tests/web/           # Frontend render tests
```

## Architecture overview

### Request/data flow

1. Browser requests `/api/jobs`
2. Rust service loads jobs from:
   - `fixtures/jobs.json` in fixture mode, or
   - Kubernetes Flink operator CRs in live mode
3. The backend normalizes raw operator resources into a typed public job DTO
4. Optional Flink REST enrichment adds a small overview summary
5. The UI renders summary cards, a job table, filters, and a details drawer

### Main backend components

- `apps/api-rs/src/main.rs` — app startup and TCP listener binding
- `apps/api-rs/src/config.rs` — env/config parsing
- `apps/api-rs/src/http/router.rs` — routes, auth gate, metrics, static serving
- `apps/api-rs/src/service/jobs_service.rs` — caching + orchestration
- `apps/api-rs/src/adapters/k8s.rs` — Kubernetes reads
- `apps/api-rs/src/adapters/flink.rs` — optional Flink REST enrichment
- `apps/api-rs/src/domain/normalize.rs` — normalization into the public DTO

### Frontend components

- `apps/web/public/index.html` — page shell
- `apps/web/public/app.js` — fetch + UI wiring
- `apps/web/public/render.js` — rendering helpers and filtering logic
- `apps/web/public/styles.css` — styling

## Prerequisites

For development:

- **Node.js** (the CI workflow uses Node 22)
- **npm**
- **Rust stable toolchain**

Optional, depending on what you want to run:

- **Docker** for image builds and smoke tests
- **Python 3** for the container smoke-check step in `npm run ci:smoke`
- access to a Kubernetes cluster running the Flink Kubernetes Operator for live mode

## Quick start

### Run locally in fixture mode

```bash
npm run dev
```

Then open:

```text
http://localhost:3000
```

Fixture mode is intentionally unauthenticated and uses `fixtures/jobs.json`.

### Run locally against a Kubernetes cluster

You can either:

1. provide `FLINK_UI_CLUSTERS_JSON`, or
2. rely on in-cluster/service-account-style `K8S_*` environment variables

Example with `FLINK_UI_CLUSTERS_JSON`:

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

## Available commands

### Development

```bash
npm run dev
```

Runs the Rust service with `FIXTURE_MODE=true`.

### Start the supported runtime

```bash
npm start
```

Delegates to:

```bash
npm run start:rust
```

### Frontend tests

```bash
npm test
```

Runs:

```bash
node --test tests/web/*.test.js
```

### Backend tests

```bash
cargo test --manifest-path apps/api-rs/Cargo.toml
```

### Production build output

```bash
npm run build
```

This creates `dist/` with:

- compiled Rust binary
- static web assets
- fixture data
- a copy of `package.json`

### Full smoke workflow

```bash
npm run ci:smoke
```

This performs:

1. Rust tests
2. frontend tests
3. Docker image build
4. fixture-mode container startup
5. HTTP smoke checks against the running container

## HTTP routes

### UI/static routes

- `GET /` — UI shell
- static assets such as `/app.js`, `/render.js`, `/styles.css`

### API routes

- `GET /api/jobs` — list normalized jobs
- `GET /api/clusters` — list distinct cluster names
- `GET /api/jobs/{id}` — fetch a job by synthetic ID
- `GET /api/jobs/{cluster}/{namespace}/{kind}/{name}` — fetch a job by locator
- `GET /api/session` — session bootstrap status for the frontend gate

### Auth/session routes

- `GET /auth/login` — start the OIDC authorization-code flow
- `GET /auth/callback` — complete the OIDC callback and mint the app session
- `POST /auth/logout` — clear the current app session

### Operational routes

- `GET /healthz` — health probe
- `GET /readyz` — readiness probe
- `GET /metrics` — JSON metrics snapshot

## Auth and security model

The app expects **application-owned OIDC/session auth** in production.

- the Rust service performs end-user login itself via the OIDC authorization-code flow
- `/auth/login`, `/auth/callback`, and `/auth/logout` manage the browser-facing auth flow
- `/api/session` lets the static UI determine whether to render the signed-out shell, loading state, or authenticated dashboard boot
- `/api/*` is protected by the app-owned same-origin session instead of ingress-injected auth headers
- the deployment must set a canonical external base URL so login redirects and callback handling stay correct behind the public host
- OIDC discovery/token/userinfo calls use a dedicated auth timeout so slow identity-provider handshakes do not inherit the tighter Kubernetes/Flink upstream timeout budget
- fixture mode bypasses OIDC only for explicit local-development runs

### Important deployment note

Do **not** expose `/metrics` on the public ingress. Keep it:

- on a separate internal-only service or scrape path, or
- behind a dedicated operations-only boundary that does not depend on browser login state

## Configuration

### Core runtime variables

| Variable                 | Purpose                                                     | Default                           |
| ------------------------ | ----------------------------------------------------------- | --------------------------------- |
| `HOST`                   | bind address                                                | `0.0.0.0`                         |
| `PORT`                   | HTTP port                                                   | `3000`                            |
| `FIXTURE_MODE`           | enable fixture-mode data source for local development only  | `false`                           |
| `FIXTURE_FILE`           | path to fixture JSON file                                   | `fixtures/jobs.json`              |
| `CACHE_TTL_MS`           | in-memory jobs cache TTL                                    | `5000`                            |
| `REQUEST_TIMEOUT_MS`     | upstream request timeout                                    | `4000`                            |
| `OIDC_ISSUER_URL`        | OIDC issuer/discovery URL                                   | none                              |
| `OIDC_CLIENT_ID`         | OIDC client identifier                                      | none                              |
| `OIDC_CLIENT_SECRET`     | OIDC client secret                                          | none                              |
| `OIDC_EXTERNAL_BASE_URL` | canonical external app URL used for redirects/callbacks     | none                              |
| `OIDC_CALLBACK_PATH`     | callback route mounted by the app                           | `/auth/callback`                  |
| `OIDC_SCOPES`            | space-delimited OIDC scopes                                 | `openid profile email`            |
| `OIDC_REQUEST_TIMEOUT_MS` | timeout for OIDC discovery/token/userinfo HTTP calls       | `max(REQUEST_TIMEOUT_MS, 15000)`  |
| `SESSION_COOKIE_SECRET`  | secret used to sign/encrypt the app session cookie          | none                              |
| `SESSION_TTL_SECS`       | app session lifetime                                        | `28800`                           |
| `SESSION_SECURE_COOKIE`  | mark the session cookie as HTTPS-only                       | `true` in production              |
| `TRUST_PROXY_HEADERS`    | honor forwarded proto/host headers from the trusted ingress | `false` unless explicitly enabled |

### Multi-cluster JSON configuration

Use `FLINK_UI_CLUSTERS_JSON` to configure one or more clusters.

Supported object fields:

| Field                   | Description                          |
| ----------------------- | ------------------------------------ |
| `name`                  | display name for the cluster         |
| `apiUrl` / `url`        | Kubernetes API base URL              |
| `bearerToken`           | inline bearer token                  |
| `bearerTokenFile`       | token file path                      |
| `caCert`                | inline CA certificate                |
| `caCertFile`            | CA certificate file path             |
| `insecureSkipTlsVerify` | disable TLS verification             |
| `namespaces`            | watched namespaces                   |
| `flinkApiVersion`       | Flink operator API version           |
| `flinkRestBaseUrl`      | explicit trusted Flink REST base URL |

### In-cluster/single-cluster environment variables

If `FLINK_UI_CLUSTERS_JSON` is not set, the app can derive a cluster from environment:

| Variable                       | Purpose                             |
| ------------------------------ | ----------------------------------- |
| `K8S_API_URL`                  | explicit Kubernetes API URL         |
| `K8S_BEARER_TOKEN`             | Kubernetes bearer token             |
| `K8S_CA_CERT`                  | Kubernetes CA certificate           |
| `K8S_CLUSTER_NAME`             | cluster display name                |
| `K8S_INSECURE_SKIP_TLS_VERIFY` | disable Kubernetes TLS verification |
| `WATCH_NAMESPACES`             | comma-separated namespaces          |
| `FLINK_K8S_API_VERSION`        | Flink operator API version          |
| `FLINK_REST_BASE_URL`          | trusted Flink REST base URL         |

When the app derives its cluster config from the in-cluster `K8S_*` / service-account path, it also auto-derives a `FlinkDeployment` JobManager UI base URL as `http://<metadata.name>-rest.<metadata.namespace>.svc:8081/` if the operator status omits `status.jobManagerUrl`. Explicit status URLs still win for UI linking, and `FlinkSessionJob` remains best-effort.

Live Flink REST enrichment fetches `/jobs/overview` only from the trusted `FLINK_REST_BASE_URL` (or the internally derived in-cluster `FlinkDeployment` service URL). Status-reported JobManager URLs are treated as display metadata for the UI; if they point outside the trusted origin, the API surfaces a warning instead of issuing an outbound request to that host.

If `K8S_API_URL` is not set, the app can derive one from:

- `KUBERNETES_SERVICE_HOST`
- `KUBERNETES_SERVICE_PORT`

It can also read service-account defaults from:

- `/var/run/secrets/kubernetes.io/serviceaccount/token`
- `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt`

## Public job contract

The public DTO includes normalized top-level fields such as:

- `id`
- `cluster`
- `namespace`
- `kind`
- `resourceName`
- `jobName`
- `status`
- `health`
- `rawStatus`
- `flinkVersion`
- `deploymentMode`
- `lastUpdatedAt`
- `startedAt`
- `flinkJobId`
- `nativeUiUrl`
- `warnings`

`details` is intentionally **sanitized** and typed. It now contains summary information only, such as:

- `statusSummary`
- `flinkRestOverview`

Raw Kubernetes `metadata`, `spec`, and `status` objects are **not** returned by default.

## Docker

Build the image:

```bash
docker build -f deploy/api/Dockerfile -t flink-job-ui:latest .
```

Run it locally in fixture mode:

```bash
docker run --rm -p 3000:3000 -e FIXTURE_MODE=true flink-job-ui:latest
```

The container image:

- builds the Rust binary in a builder stage
- copies static assets and fixtures into the final image
- runs `./apps/api-rs/flink-job-ui-api`

## Kubernetes deployment

Example manifests live under:

- `deploy/api/deployment.yaml`
- `deploy/api/service.yaml`
- `deploy/api/ingress.yaml`

The included deployment assumes:

- Rust is the only supported production runtime
- the app listens on port `3000`
- the application owns browser-facing OIDC login/session auth in production; ingress should only forward traffic and preserve the canonical HTTPS host
- `deploy/api/deployment.yaml` is a **local port-forward example** until you replace `OIDC_EXTERNAL_BASE_URL=http://localhost:3000` and `SESSION_SECURE_COOKIE=false` with the real HTTPS ingress settings
- `/metrics` should not be publicly exposed without protection
- same-domain JobManager UI proxying depends on the app pod being able to reach the in-cluster JobManager REST service; when `status.jobManagerUrl` is missing, the app derives `FlinkDeployment` URLs as `http://<name>-rest.<namespace>.svc:8081/`
- the v1 JobManager proxy stays read-only and does not support websocket/upgrade flows

## CI

GitHub Actions workflow:

- `.github/workflows/ci-smoke.yml`

Current CI behavior:

- runs on pushes to `main`/`master`
- runs on pull requests
- sets up Node + Rust
- executes `npm run ci:smoke`

## Known limitations

- read-only dashboard only; no suspend/cancel/savepoint actions
- local dev defaults to fixture mode rather than a live cluster
- Flink REST enrichment is best-effort and should never block job listing
- Flink REST enrichment only calls trusted configured origins (`flinkRestBaseUrl` / `FLINK_REST_BASE_URL`) or the server-derived in-cluster `FlinkDeployment` service URL; status-derived URLs are warning-only for enrichment decisions
- JobManager UI proxying is read-only and best suited for in-cluster or otherwise app-reachable JobManager URLs; websocket/upgrade flows are not supported in v1
- `FlinkSessionJob` collection is also best-effort
- production access control depends on correct ingress/reverse-proxy configuration

## Additional docs

- `docs/architecture/flink-job-ui-v1.md`
- `docs/ops/local-dev.md`

## License

Do What The F*ck You Want To Public License
