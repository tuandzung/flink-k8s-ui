# Flink Job UI v2 Baseline Architecture

This document describes the current v2 baseline for the Flink Job UI: the original
status dashboard plus authenticated single-resource control actions
(`cancel`, `suspend`, `resume`).

## Scope

- dashboard for listing Flink jobs and status
- support `FlinkDeployment` and `FlinkSessionJob`
- rely on Kubernetes operator CRs first, optional Flink REST enrichment second
- support authenticated single-resource `cancel`, `suspend`, and `resume` actions
- refresh the authoritative resource state immediately after every action

## Why this shape

The implementation optimizes for:

- minimal dependencies
- a single Rust service that owns both shell delivery and protected APIs
- easy local development via fixture mode
- a clear adapter boundary between Kubernetes discovery, action mutation, and UI rendering

## Components

### Backend

- `apps/api-rs/src/http/router.rs` — HTTP router, metrics, auth gate, and static shell asset serving
- `apps/api-rs/src/http/handlers/jobs.rs` — protected list + action handlers for job resources
- `apps/api-rs/src/service/jobs_service.rs` — cached job aggregation and post-action refresh lookup
- `apps/api-rs/src/adapters/k8s.rs` — Kubernetes reads + action mutations
- `apps/api-rs/src/adapters/flink.rs` — optional Flink REST enrichment
- `apps/api-rs/src/domain/normalize.rs` — normalization into canonical UI states/action affordances

### Frontend

- `apps/web/public/index.html` — signed-out shell and authenticated layout frame
- `apps/web/public/app.js` — auth bootstrap, job fetch, action submission, and DOM wiring
- `apps/web/public/render.js` — rendering helpers, filter logic, and action controls/feedback
- `apps/web/public/styles.css` — styling

## Data flow

1. The browser requests `/` and loads the current versioned shell assets.
2. The frontend calls `GET /api/session` to discover whether an authenticated session exists.
3. Once authenticated, the UI calls `GET /api/jobs`.
4. The backend loads fixture data or reads operator CRs from Kubernetes.
5. The backend normalizes resource status into canonical UI states and may synthesize an in-cluster `FlinkDeployment` JobManager URL (`http://<name>-rest.<namespace>.svc:8081/`) when operator status omits one.
6. The backend optionally enriches results from Flink REST.
7. The UI renders summary cards, filters, the table, the details drawer, and action controls.
8. Action submissions call the protected locator-based action endpoint and immediately refetch so the table and drawer reflect the authoritative post-action state.

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
- CSRF protection stays enforced for every mutating action request

## Configuration

- `FIXTURE_MODE=true` uses `fixtures/jobs.json`
- `FLINK_UI_CLUSTERS_JSON` supports explicit cluster config
- `K8S_*` env vars support a single-cluster deployment
- the in-cluster auto-derived cluster path also enables `FlinkDeployment` JobManager URL discovery via the `<name>-rest.<namespace>.svc:8081` service convention
- shell HTML is served as the authoritative source for current JS/CSS asset versions; browsers cache versioned assets immutably

## Protection model

- The supported production runtime is the Rust service in `apps/api-rs`; both `npm start` and the example Kubernetes deployment execute that binary.
- End-user authentication is owned by the application process via OIDC login, callback, logout, and a same-origin session cookie.
- The signed-out shell and auth endpoints stay public enough to start the login flow, while `/api/*` is gated on the app-owned session.
- `/metrics` is an operational endpoint. Keep it off the public ingress and expose it only on an internal-only service or scrape path.
- Local fixture-mode development is intentionally ungated so a single developer can run the UI without provisioning SSO.

## Known baseline limits

- bulk actions and savepoints remain out of scope
- fixture mode still renders action affordances but does not execute live mutations
- JobManager UI proxying remains read-only; websocket/upgrade flows are not supported
- `FlinkSessionJob` collection remains best-effort
- production authorization rules beyond session authentication are still a future layer

## Historical note

The original read-only architecture note now lives at `docs/architecture/flink-job-ui-v1.md`
as a short historical reference. This v2 document is the canonical architecture note for the
current branch/runtime.
