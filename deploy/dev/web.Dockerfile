# Development image of the Angular front end (ng serve, ng test, client
# generation). Used by the `web` service of deploy/compose.dev.yaml.
# Playwright gets its own image with L01-US07.

FROM node:24-trixie

# Commands run with the host user's uid (see the justfile). The npm cache
# lives in a named volume; Docker copies these permissions on first use.
ENV HOME=/home/dev \
    npm_config_cache=/npm-cache \
    NG_CLI_ANALYTICS=false
RUN mkdir -p /home/dev /npm-cache \
    && chmod 1777 /home/dev /npm-cache

WORKDIR /workspace/web
