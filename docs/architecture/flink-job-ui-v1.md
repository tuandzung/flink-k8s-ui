# Flink Job UI v1 Architecture (Historical)

This note captures the original v1 shape of the project: a read-only dashboard for listing
Flink jobs and surfacing normalized status/warning data.

The current product baseline is **v2**, which adds authenticated single-resource
`cancel`, `suspend`, and `resume` actions on top of that foundation.

## Current canonical docs

- `docs/architecture/flink-job-ui-v2.md` — current architecture for the v2 baseline
- `docs/ops/local-dev.md` — local development and deployment notes

## What v1 referred to

- read-only listing/filtering for `FlinkDeployment` and `FlinkSessionJob`
- normalized status, warnings, and details drawer rendering
- optional Flink REST enrichment
- no authenticated resource mutation actions

Use the v2 architecture note for the current system design and runtime behavior.
