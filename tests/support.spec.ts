import { expect, test, type Page } from '@playwright/test';
import { runCommand } from './rail-helper';

/** Help & support has no rail button: it is a command in Go to anything. */
const support = (page: Page) => runCommand(page, 'Help & support');
const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent' });

async function open(page: Page, behavior = 'mock'): Promise<void> {
  await page.addInitScript(({ behavior }) => {
    (window as any).__supportFixture = { behavior, calls: 0, opened: [] };
    window.open = (...args: any[]) => { (window as any).__supportFixture.opened.push(args); return null; };
  }, { behavior });
  await page.route('**/src/preview.ts*', async route => {
    const response = await route.fetch();
    const source = await response.text();
    const marker = /switch\s*\(command\)\s*\{/;
    expect(source).toMatch(marker);
    await route.fulfill({ response, body: source.replace(marker, `
      const supportFixture = window.__supportFixture;
      if (command === 'open_support') {
        supportFixture.calls++;
        supportFixture.args = args;
        if (supportFixture.behavior === 'hold') await new Promise(resolve => { supportFixture.resolve = resolve; });
        if (supportFixture.behavior === 'fail') throw new Error('Could not open support. Please try again.');
        if (supportFixture.behavior !== 'preview') return;
      }
      switch (command) {
    `) });
  });
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await expect(prompt(page)).toBeEnabled();
}

const calls = (page: Page) => page.evaluate(() => (window as any).__supportFixture.calls);

test('Help is a command in Go to anything and opens only when asked', async ({ page }) => {
  await open(page);
  await expect(page.locator('.sidebar-bottom [data-action="support"], #support-button')).toHaveCount(0);
  expect(await calls(page)).toBe(0);
  // From the keyboard: ⌘K, its name, Enter.
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('textbox', { name: 'Search tasks and commands', exact: true }).fill('help');
  await expect(page.locator('#command-results').getByRole('button').first()).toHaveText(/Help & support/);
  await page.keyboard.press('Enter');
  await expect.poll(() => calls(page)).toBe(1);
  expect(await page.evaluate(() => (window as any).__supportFixture.args)).toEqual({});
});

test('a second request while support is opening does not open it twice', async ({ page }) => {
  await open(page, 'hold');
  await support(page);
  await expect.poll(() => calls(page)).toBe(1);
  await support(page);
  await page.waitForTimeout(300);
  expect(await calls(page)).toBe(1);
  await page.evaluate(() => (window as any).__supportFixture.resolve());
});

test('support recovers after a failed browser launch', async ({ page }) => {
  await open(page, 'fail');
  await support(page);
  await expect(page.locator('#toast')).toHaveText('Could not open support. Please try again.');
  await page.evaluate(() => { (window as any).__supportFixture.behavior = 'mock'; });
  await support(page);
  await expect.poll(() => calls(page)).toBe(2);
});

test('preview opens the GitHub issue tracker without sending customer or workspace details', async ({ page }) => {
  await open(page, 'preview');
  await support(page);
  await expect.poll(() => page.evaluate(() => (window as any).__supportFixture.opened)).toEqual([
    ['https://github.com/aicodeking/pandacode/issues', '_blank', 'noopener,noreferrer'],
  ]);
});

test('Help stays visible in narrow windows with large interface text', async ({ page }) => {
  await page.setViewportSize({ width: 460, height: 740 });
  await page.addInitScript(() => localStorage.setItem('pandacode.uiScale', '1.6'));
  await open(page);
  for (const mode of ['sidebar', 'collapsed', 'board']) {
    if (mode === 'collapsed') await page.getByRole('button', { name: 'Hide sidebar', exact: true }).click();
    if (mode === 'board') { await page.getByRole('button', { name: /^Workspace view: / }).click(); await page.getByRole('menuitemradio', { name: 'Board', exact: true }).click(); }
    await expect(support(page)).toBeVisible();
    const rect = await support(page).boundingBox();
    expect(rect!.width).toBeGreaterThanOrEqual(28);
    expect(rect!.x).toBeGreaterThanOrEqual(0);
    expect(rect!.x + rect!.width).toBeLessThanOrEqual(460);
    expect(rect!.y + rect!.height).toBeLessThanOrEqual(740);
    await support(page).focus();
    await expect(support(page)).toBeFocused();
  }
});

test('Settings shows an About section with the version and GitHub links', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  await settings.getByRole('tab', { name: 'About', exact: true }).click();
  const about = settings.locator('.settings-about');
  await expect(about).toBeVisible();
  await expect(about.locator('.settings-about-version')).toHaveText('Version Preview');
  const urls = await about.locator('[data-action="open-url"]').evaluateAll(nodes => nodes.map(node => (node as HTMLElement).dataset.url));
  expect(urls).toEqual(expect.arrayContaining([
    'https://github.com/aicodeking/pandacode',
    'https://github.com/aicodeking/pandacode/issues',
    'https://github.com/aicodeking/pandacode/releases',
  ]));
  await expect(about.getByRole('button', { name: 'Releases', exact: true })).toBeVisible();
  await expect(about.getByRole('button', { name: 'PandaCode License' })).toBeVisible();
  await expect(settings).not.toContainText('License & updates');
  await about.getByRole('button', { name: 'Releases', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__supportFixture.opened)).toEqual([
    ['https://github.com/aicodeking/pandacode/releases', '_blank', 'noopener,noreferrer'],
  ]);
});
