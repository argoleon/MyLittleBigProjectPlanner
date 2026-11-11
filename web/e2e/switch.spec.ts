/**
 * Switching between the member's tenants (L02-US05), API mocked: the
 * drawer lists them with the member's role, picking one leaves the page for
 * the link the API issues, and the other tenant's application loads afresh.
 */
import { MEMBERSHIP, SESSION, accessibilityViolations, expect, problem, test } from './fixtures';

const GLOBEX = {
  tenantId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a90',
  slug: 'globex',
  name: 'Globex',
  url: 'https://globex.planner.example',
  accountType: 'internal',
  tenantRole: 'tenant_admin',
  current: false,
};

test.describe('tenant switcher', () => {
  test('lists the member’s tenants with the role in each, accessibly', async ({ page }) => {
    await page.route('**/api/v1/me/memberships', (route) => route.fulfill({ json: [MEMBERSHIP, GLOBEX] }));
    await page.goto('/');
    const switcher = page.getByRole('combobox', { name: 'Organization' });
    await expect(switcher).toContainText(MEMBERSHIP.name);
    await switcher.click();
    await expect(page.getByRole('option')).toHaveText([`${MEMBERSHIP.name}Member`, `${GLOBEX.name}Organization admin`]);
    await expect(page.getByRole('option', { name: /^Acme/ })).toHaveAttribute('aria-selected', 'true');
    expect(await accessibilityViolations(page)).toEqual([]);
  });

  test('leaves for the other tenant, which loads afresh', async ({ page, baseURL }) => {
    let switched = false;
    await page.route('**/api/v1/me/memberships', (route) =>
      route.fulfill({
        json: switched ? [{ ...MEMBERSHIP, current: false }, { ...GLOBEX, current: true }] : [MEMBERSHIP, GLOBEX],
      }),
    );
    await page.route('**/api/v1/auth/session', (route) =>
      route.fulfill({
        json: switched ? { ...SESSION, account: { ...SESSION.account, tenantRole: 'tenant_admin' } } : SESSION,
      }),
    );
    let body: unknown;
    await page.route('**/api/v1/me/switch', (route) => {
      body = route.request().postDataJSON();
      // The real link is on the target's host; here, the same origin.
      return route.fulfill({ json: { url: `${baseURL}/api/v1/auth/switch?token=one-time` } });
    });
    await page.route('**/api/v1/auth/switch?token=one-time', (route) => {
      switched = true;
      return route.fulfill({ status: 303, headers: { location: '/' } });
    });

    await page.goto('/workload');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
    // Marks this page: a full load forgets it.
    await page.evaluate(() => ((window as unknown as { leftBehind: boolean }).leftBehind = true));
    await page.getByRole('combobox', { name: 'Organization' }).click();
    await page.getByRole('option', { name: /^Globex/ }).click();

    await expect(page.getByRole('combobox', { name: 'Organization' })).toContainText(GLOBEX.name);
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
    expect(body).toEqual({ tenantId: GLOBEX.tenantId });
    expect(await page.evaluate(() => (window as unknown as { leftBehind?: boolean }).leftBehind)).toBeUndefined();
  });

  test('stays on the current tenant with a message when the switch is refused', async ({ page }) => {
    await page.route('**/api/v1/me/memberships', (route) => route.fulfill({ json: [MEMBERSHIP, GLOBEX] }));
    await page.route('**/api/v1/me/switch', (route) => route.fulfill(problem(403, 'forbidden')));
    await page.goto('/');
    await page.getByRole('combobox', { name: 'Organization' }).click();
    await page.getByRole('option', { name: /^Globex/ }).click();
    await expect(page.getByText('You are not allowed to do this.')).toBeVisible();
    await expect(page.getByRole('combobox', { name: 'Organization' })).toContainText(MEMBERSHIP.name);
  });
});
