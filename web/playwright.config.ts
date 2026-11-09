import { defineConfig, devices } from '@playwright/test';

/**
 * End-to-end tests of the internal application, run by `just e2e` in the
 * e2e container. The production build is served by `ng serve`; the API is
 * mocked per test with `page.route`, so no backend is needed.
 */
const PORT = 4300;

export default defineConfig({
  testDir: 'e2e',
  outputDir: 'dist/e2e/results',
  fullyParallel: true,
  forbidOnly: !!process.env['CI'],
  retries: process.env['CI'] ? 1 : 0,
  reporter: [['list'], ['html', { outputFolder: 'dist/e2e/report', open: 'never' }]],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `npx ng serve app --configuration production --host 127.0.0.1 --port ${PORT}`,
    url: `http://127.0.0.1:${PORT}`,
    timeout: 180_000,
    reuseExistingServer: false,
  },
});
