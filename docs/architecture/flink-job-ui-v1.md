# Flink Job UI Architecture

> This document started as the v1 read-only architecture note. It now also captures the v2 baseline single-resource control actions (`cancel`, `suspend`, `resume`) added on top of that foundation.

## Scope

- dashboard for listing Flink jobs and status
- support `FlinkDeployment` and `FlinkSessionJob`
- rely on Kubernetes operator CRs first, optional Flink REST enrichment second
- support single-resource `cancel`, `suspend`, and `resume` actions through operator CR semantics

## Why this shape

The repo was greenfield, so the implementation optimizes for:

- minimal dependencies
- easy local development via fixture mode
- a clean adapter boundary between Kubernetes discovery and UI rendering

## Components

### Backend

- `apps/api-rs/src/http/router.rs` — HTTP router, metrics, and static asset serving
- `apps/api-rs/src/http/handlers/jobs.rs` — read + action handlers for job resources
- `apps/api-rs/src/service/jobs_service.rs` — cached job aggregation
- `apps/api-rs/src/adapters/k8s.rs` — Kubernetes reads + action mutations
- `apps/api-rs/src/adapters/flink.rs` — optional Flink REST enrichment

### Frontend

- `apps/web/public/index.html` — shell
- `apps/web/public/app.js` — fetch, action submission, and post-action refresh wiring
- `apps/web/public/render.js` — rendering helpers, filter logic, and action controls/feedback

## Data flow

1. UI calls `GET /api/jobs`
2. backend loads fixture data or reads operator CRs from Kubernetes
3. backend normalizes resource status into canonical UI states and may synthesize an in-cluster `FlinkDeployment` JobManager URL (`http://<name>-rest.<namespace>.svc:8081/`) when operator status omits one
4. backend optionally enriches results from Flink REST
5. UI renders summary cards, filters, table, and details drawer
6. Action submissions call the protected locator-based action endpoint and force an immediate refetch so the drawer/table reflect the authoritative post-action state

## Canonical status vocabulary

- `running`
- `reconciling`
- `suspended`
- `failed`
- `unknown`

Raw status is still preserved in the details view for troubleshooting.

## Action semantics

- `suspend` patches the operator-managed resource into a suspended state
- `resume` is only available for suspended resources
- `cancel` deletes/removes the operator-managed resource and is terminal
- unsupported action/state combinations stay disabled in the UI and return `409`-style API responses if called directly

## Configuration

- `FIXTURE_MODE=true` uses `fixtures/jobs.json`
- `FLINK_UI_CLUSTERS_JSON` supports explicit cluster config
- `K8S_*` env vars support a single-cluster deployment
- the in-cluster auto-derived cluster path also enables `FlinkDeployment` JobManager URL discovery via the `<name>-rest.<namespace>.svc:8081` service convention

## Protection model

- The supported production runtime is the Rust service in `apps/api-rs`; both `npm start` and the example Kubernetes deployment execute that binary.
- End-user authentication is owned by the application process via OIDC login, callback, logout, and a same-origin session cookie.
- The signed-out shell and auth endpoints stay public enough to start the login flow, while `/api/*` is gated on the app-owned session.
- `/metrics` is an operational endpoint. Keep it off the public ingress and expose it only on an internal-only service or scrape path.
- Local fixture-mode development is intentionally ungated so a single developer can run the UI without provisioning SSO.

## Next extension points

- namespace or cluster-level watches instead of polling
- authorization and identity-aware business logic on top of the session layer
- savepoints and richer operator workflows after the baseline action surface is stable
