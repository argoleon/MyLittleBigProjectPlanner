import { defineConfig, devices } from '@playwright/test';

/**
 * End-to-end tests of the internal application, run in the e2e container.
 *
 * - `just e2e`: the production build served by `ng serve`.
 * - `just compose-e2e`: the production stack (nginx behind Traefik, with its
 *   Content-Security-Policy), when E2E_BASE_URL is set. E2E_HOST_RULES maps
 *   that URL's host to the Traefik container (Chromium host resolver rules).
 *
 * The API is mocked per test with `page.route`, so no data is needed.
 */
const PORT = 4300;
const external = process.env['E2E_BASE_URL'];
const hostRules = process.env['E2E_HOST_RULES'];

export default defineConfig({
  testDir: 'e2e',
  outputDir: 'dist/e2e/results',
  fullyParallel: true,
  forbidOnly: !!process.env['CI'],
  retries: process.env['CI'] ? 1 : 0,
  reporter: [['list'], ['html', { outputFolder: 'dist/e2e/report', open: 'never' }]],
  use: {
    baseURL: external ?? `http://127.0.0.1:${PORT}`,
    // The local stack uses Traefik's self-signed certificate.
    ignoreHTTPSErrors: !!external,
    // Kept as CI artifacts (web/dist/e2e/) when a test fails.
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: hostRules ? { args: [`--host-resolver-rules=${hostRules}`] } : {},
      },
    },
  ],
  webServer: external
    ? undefined
    : {
        command: `npx ng serve app --configuration production --host 127.0.0.1 --port ${PORT}`,
        url: `http://127.0.0.1:${PORT}`,
        timeout: 180_000,
        reuseExistingServer: false,
      },
});
