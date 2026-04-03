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
- **Production auth model:** upstream ingress or reverse proxy is expected to authenticate requests before they reach the app
- **Protected production routes:** `/api/*` and `/metrics`
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

### Operational routes

- `GET /healthz` — health probe
- `GET /readyz` — readiness probe
- `GET /metrics` — JSON metrics snapshot

## Auth and security model

The app currently expects **upstream auth ownership** in production.

- the Rust service does **not** perform end-user login or SSO itself
- `/api/*` and `/metrics` are protected by a trusted-header gate in live mode
- by default, trusted headers are:
  - `x-auth-request-user`
  - `x-forwarded-user`
- ingress/reverse proxy should inject one of those headers after authenticating the caller
- fixture mode bypasses the gate for local development

### Important deployment note

Do **not** expose `/metrics` anonymously. Keep it:

- behind the same trusted ingress auth boundary, or
- on a separate internal-only scrape path

## Configuration

### Core runtime variables

| Variable | Purpose | Default |
|---|---|---|
| `HOST` | bind address | `0.0.0.0` |
| `PORT` | HTTP port | `3000` |
| `FIXTURE_MODE` | enable fixture-mode data source | auto-enabled when no live cluster config is present |
| `FIXTURE_FILE` | path to fixture JSON file | `fixtures/jobs.json` |
| `CACHE_TTL_MS` | in-memory jobs cache TTL | `5000` |
| `REQUEST_TIMEOUT_MS` | upstream request timeout | `4000` |
| `AUTH_TRUSTED_HEADERS` | comma-separated trusted auth header names | `x-auth-request-user,x-forwarded-user` |

### Multi-cluster JSON configuration

Use `FLINK_UI_CLUSTERS_JSON` to configure one or more clusters.

Supported object fields:

| Field | Description |
|---|---|
| `name` | display name for the cluster |
| `apiUrl` / `url` | Kubernetes API base URL |
| `bearerToken` | inline bearer token |
| `bearerTokenFile` | token file path |
| `caCert` | inline CA certificate |
| `caCertFile` | CA certificate file path |
| `insecureSkipTlsVerify` | disable TLS verification |
| `namespaces` | watched namespaces |
| `flinkApiVersion` | Flink operator API version |
| `flinkRestBaseUrl` | explicit trusted Flink REST base URL |

### In-cluster/single-cluster environment variables

If `FLINK_UI_CLUSTERS_JSON` is not set, the app can derive a cluster from environment:

| Variable | Purpose |
|---|---|
| `K8S_API_URL` | explicit Kubernetes API URL |
| `K8S_BEARER_TOKEN` | Kubernetes bearer token |
| `K8S_CA_CERT` | Kubernetes CA certificate |
| `K8S_CLUSTER_NAME` | cluster display name |
| `K8S_INSECURE_SKIP_TLS_VERIFY` | disable Kubernetes TLS verification |
| `WATCH_NAMESPACES` | comma-separated namespaces |
| `FLINK_K8S_API_VERSION` | Flink operator API version |
| `FLINK_REST_BASE_URL` | trusted Flink REST base URL |

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
- ingress or reverse proxy authenticates public traffic
- `/metrics` should not be publicly exposed without protection

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
- `FlinkSessionJob` collection is also best-effort
- production access control depends on correct ingress/reverse-proxy configuration

## Additional docs

- `docs/architecture/flink-job-ui-v1.md`
- `docs/ops/local-dev.md`

## License

No license file is currently included in this repository.
