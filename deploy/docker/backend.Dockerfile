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
# - The dependencies are compiled in a layer of their own (cargo-chef): it
#   only changes with Cargo.lock, so a layer cache (CI: a registry cache,
#   see deploy/compose.ci-cache.yaml) leaves only our crates to compile.
#   No cache mount: what they hold is not part of a layer cache.

ARG RUST_IMAGE=rust:1.99.1-trixie
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot

FROM ${RUST_IMAGE} AS chef
RUN cargo install --locked cargo-auditable@0.7.7 cargo-chef@0.1.78 \
    && rm -rf /usr/local/cargo/registry
WORKDIR /src

# The recipe: manifests and lock file only, the same while they are.
FROM chef AS plan
COPY backend/ ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p api -p worker -p mcp --recipe-path recipe.json
COPY backend/ ./
ENV SQLX_OFFLINE=true
RUN cargo auditable build --release --locked -p api -p worker -p mcp \
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
