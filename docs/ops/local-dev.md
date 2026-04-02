# Local Development

## Run in fixture mode
```bash
npm run dev
```

Open `http://localhost:3000`.

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
Provide either:

1. `FLINK_UI_CLUSTERS_JSON` with one or more cluster objects, or
2. in-cluster variables such as `KUBERNETES_SERVICE_HOST` plus mounted service account token

Example:
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

## Notes
- The default `npm start` / `npm run dev` path now runs the Rust backend in `apps/api-rs`.
- `FlinkSessionJob` collection is best-effort; clusters without that CR kind still work.
- Flink REST enrichment is optional and never blocks job listing.
- There is no separate Node backend runtime path anymore.
