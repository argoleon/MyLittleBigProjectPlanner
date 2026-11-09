# syntax=docker/dockerfile:1
#
# Production images of the Rust services: one shared build, then one small
# image per binary (targets api, worker, mcp).
#
#   docker build -f deploy/docker/backend.Dockerfile --target api -t mlbpp-api .
#
# - Built with `cargo auditable`: the dependency list is embedded in each
#   binary, so the SBOM of the image (syft) lists the crates.
# - Runtime: distroless (glibc, CA certificates, no shell), user nonroot.
# - Queries are checked against backend/.sqlx: no database needed to build.
# - Migrations are embedded in the api binary: `api migrate` runs them.

ARG RUST_IMAGE=rust:1.99.1-trixie
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot

FROM ${RUST_IMAGE} AS build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install --locked cargo-auditable@0.7.7
WORKDIR /src
COPY backend/ ./
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo auditable build --release --locked -p api -p worker -p mcp \
    && mkdir -p /out \
    && cp target/release/api target/release/worker target/release/mcp /out/

FROM ${RUNTIME_IMAGE} AS runtime
LABEL org.opencontainers.image.source="https://github.com/argoleon/MyLittleBigProjectPlanner" \
      org.opencontainers.image.licenses="MIT"
USER nonroot:nonroot

FROM runtime AS api
LABEL org.opencontainers.image.title="mlbpp-api" \
      org.opencontainers.image.description="My Little Big Project Planner REST API"
COPY --from=build /out/api /usr/local/bin/api
ENV API_BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 CMD ["/usr/local/bin/api", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/api"]

FROM runtime AS worker
LABEL org.opencontainers.image.title="mlbpp-worker" \
      org.opencontainers.image.description="My Little Big Project Planner background jobs"
COPY --from=build /out/worker /usr/local/bin/worker
ENV WORKER_HEALTH_ADDR=0.0.0.0:8081
EXPOSE 8081
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 CMD ["/usr/local/bin/worker", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/worker"]

FROM runtime AS mcp
LABEL org.opencontainers.image.title="mlbpp-mcp" \
      org.opencontainers.image.description="My Little Big Project Planner MCP server"
COPY --from=build /out/mcp /usr/local/bin/mcp
ENV MCP_BIND_ADDR=0.0.0.0:8090
EXPOSE 8090
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 CMD ["/usr/local/bin/mcp", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/mcp"]
