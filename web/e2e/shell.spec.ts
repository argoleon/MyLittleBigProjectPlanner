import { TENANT, accessibilityViolations, expect, test } from './fixtures';

test.describe('application shell', () => {
  test('loads fonts and icons from its own origin only', async ({ page, baseURL }) => {
    const requests: string[] = [];
    page.on('request', (request) => requests.push(request.url()));

    await page.goto('/');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
    await page.evaluate(() => document.fonts.ready);
    await page.getByRole('link', { name: 'Workload' }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
    await page.waitForLoadState('networkidle');

    const origin = new URL(baseURL!).origin;
    const foreign = requests.filter((url) => !url.startsWith('data:') && new URL(url).origin !== origin);
    expect(foreign).toEqual([]);
    // The fonts were really requested, and from our origin.
    expect(requests.some((url) => /\/media\/roboto-latin-400-normal-.*\.woff2$/.test(url))).toBe(true);
    expect(requests.some((url) => /\/media\/material-symbols-outlined-.*\.woff2$/.test(url))).toBe(true);
    const fonts = await page.evaluate(() => ({
      roboto: document.fonts.check('16px Roboto'),
      symbols: document.fonts.check('24px "Material Symbols Outlined"'),
    }));
    expect(fonts).toEqual({ roboto: true, symbols: true });
  });

  test('shows the tenant from the API and highlights the current screen', async ({ page }) => {
    await page.goto('/gantt');
    await expect(page.getByRole('combobox', { name: 'Organization' })).toContainText(TENANT.name);
    const current = page.getByRole('link', { name: 'Gantt' });
    await expect(current).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('link', { name: 'Home' })).not.toHaveAttribute('aria-current', 'page');
    await expect(page).toHaveTitle('Gantt · My Little Big Project Planner');
  });

  test('can be used with the keyboard alone', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

    // The skip link comes first and becomes visible when focused.
    await page.keyboard.press('Tab');
    const skip = page.getByRole('link', { name: 'Skip to content' });
    await expect(skip).toBeFocused();
    await expect(skip).toBeInViewport();
    await page.keyboard.press('Enter');
    await expect(page.locator('main')).toBeFocused();

    // From the top of a fresh page: skip link, organization, then the navigation.
    await page.reload();
    await expect(page.getByRole('combobox')).toContainText(TENANT.name);
    await page.keyboard.press('Tab');
    await page.keyboard.press('Tab');
    await expect(page.getByRole('combobox', { name: 'Organization' })).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(page.getByRole('link', { name: 'Home' })).toBeFocused();
    await page.keyboard.press('Tab');
    const tickets = page.getByRole('link', { name: 'My tickets' });
    await expect(tickets).toBeFocused();
    // The focus is visible: the focused entry has a focus indicator.
    const indicator = await tickets.evaluate((link) => {
      const style = getComputedStyle(link, '::before');
      return style.content !== 'none' && style.opacity !== '0';
    });
    expect(indicator).toBe(true);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('My tickets');
  });

  test('switches language from the top app bar', async ({ page }) => {
    await page.goto('/workload');
    await page.getByRole('button', { name: 'Language: English' }).click();
    await page.getByRole('menuitemradio', { name: 'Français' }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Charge');
    await expect(page.getByRole('navigation', { name: 'Navigation principale' })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr');
    await expect(page).toHaveTitle('Charge · My Little Big Project Planner');

    // The choice survives a reload.
    await page.reload();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Charge');
  });

  test('switches between light and dark mode, colors coming from the theme tokens', async ({ page }) => {
    await page.goto('/');
    const colors = () =>
      page.evaluate(() => {
        const probe = document.createElement('div');
        probe.style.color = 'var(--mat-sys-surface)';
        document.body.append(probe);
        const surface = getComputedStyle(probe).color;
        probe.remove();
        return { body: getComputedStyle(document.body).backgroundColor, surface };
      });

    await page.getByRole('button', { name: /^Theme:/ }).click();
    await page.getByRole('menuitemradio', { name: 'Light' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    const light = await colors();

    await page.getByRole('button', { name: 'Theme: Light' }).click();
    await page.getByRole('menuitemradio', { name: 'Dark' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    const dark = await colors();

    expect(light.body).toBe(light.surface);
    expect(dark.body).toBe(dark.surface);
    expect(dark.body).not.toBe(light.body);
  });
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the shell has no axe-core violation', async ({ page }) => {
        await page.goto('/workload');
        await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
        await expect(page.getByRole('combobox')).toContainText(TENANT.name);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the open menus have no axe-core violation', async ({ page }) => {
        await page.goto('/');
        await page.getByRole('button', { name: /^(Theme|Thème)/ }).click();
        await expect(page.getByRole('menu')).toBeVisible();
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}

test('narrow screens get a modal navigation drawer', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  const navigation = page.getByRole('navigation', { name: 'Main navigation' });
  await expect(navigation).toBeHidden();
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await expect(navigation).toBeVisible();
  await navigation.getByRole('link', { name: 'Search' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Search');
  await expect(navigation).toBeHidden();
  expect(await accessibilityViolations(page)).toEqual([]);
});
