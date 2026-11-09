# End-to-end tests (Playwright, axe-core) of the Angular applications.
# The image version must match @playwright/test in web/package.json: it
# ships the browsers that version drives.

FROM mcr.microsoft.com/playwright:v1.65.0-noble

# Commands run with the host user's uid (see the justfile).
ENV HOME=/home/dev \
    npm_config_cache=/npm-cache \
    NG_CLI_ANALYTICS=false
RUN mkdir -p /home/dev /npm-cache \
    && chmod 1777 /home/dev /npm-cache

WORKDIR /workspace/web
