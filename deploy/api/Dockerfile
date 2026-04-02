FROM rust:1.94-bookworm AS build
WORKDIR /app

COPY package.json ./package.json
COPY apps/api-rs/Cargo.toml ./apps/api-rs/Cargo.toml
COPY apps/api-rs/src ./apps/api-rs/src
COPY apps/web/public ./apps/web/public
COPY apps/api/src/fixtures ./apps/api/src/fixtures

RUN cargo build --manifest-path apps/api-rs/Cargo.toml --release

FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/package.json ./package.json
COPY --from=build /app/apps/web/public ./apps/web/public
COPY --from=build /app/apps/api/src/fixtures ./apps/api/src/fixtures
COPY --from=build /app/apps/api-rs/target/release/flink-job-ui-api ./apps/api-rs/flink-job-ui-api

ENV HOST=0.0.0.0
ENV PORT=3000

CMD ["./apps/api-rs/flink-job-ui-api"]
