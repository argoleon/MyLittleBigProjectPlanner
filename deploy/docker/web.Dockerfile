# syntax=docker/dockerfile:1
#
# Production image of the front end: both Angular applications, built once,
# served by nginx as static files (non-root, port 8080).
#
#   /          internal application (projects/app)
#   /portal/   request portal (projects/portal)
#
#   docker build -f deploy/docker/web.Dockerfile -t mlbpp-web .

ARG NODE_IMAGE=node:24-trixie
ARG RUNTIME_IMAGE=nginxinc/nginx-unprivileged:1.30-alpine

FROM ${NODE_IMAGE} AS build
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci --no-audit --no-fund
COPY web/ ./
RUN npx ng build app \
    && npx ng build portal --base-href /portal/

FROM ${RUNTIME_IMAGE}
LABEL org.opencontainers.image.title="mlbpp-web" \
      org.opencontainers.image.description="My Little Big Project Planner web applications" \
      org.opencontainers.image.source="https://github.com/argoleon/MyLittleBigProjectPlanner" \
      org.opencontainers.image.licenses="MIT"
COPY deploy/docker/nginx/default.conf /etc/nginx/conf.d/default.conf
COPY deploy/docker/nginx/security-headers.conf /etc/nginx/snippets/security-headers.conf
COPY --from=build /src/web/dist/app/browser /usr/share/nginx/html
COPY --from=build /src/web/dist/portal/browser /usr/share/nginx/html/portal
# Not served: lets the image SBOM (syft) list the npm dependencies.
COPY web/package.json web/package-lock.json /usr/share/doc/mlbpp-web/
# 8080: the applications; 8081: the liveness probe only.
EXPOSE 8080 8081
HEALTHCHECK --interval=10s --timeout=5s --start-period=5s --retries=3 \
    CMD wget -q --spider http://127.0.0.1:8081/health/live || exit 1
