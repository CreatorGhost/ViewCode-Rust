import { expect, test, type Page } from '@playwright/test';
import { runCommand } from './rail-helper';

const repository = 'https://github.com/aicodeking/pandacode';
const brand = (page: Page) => page.getByRole('button', { name: 'Replay PandaCode animation', exact: true });
const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent' });

async function open(page: Page): Promise<void> {
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await expect(prompt(page)).toBeEnabled({ timeout: 20_000 });
}

async function recordWindowOpens(page: Page): Promise<void> {
  await page.addInitScript(() => {
    (window as any).__externalUrlOpens = [];
    window.open = ((url?: string | URL) => {
      (window as any).__externalUrlOpens.push(String(url ?? ''));
      return null;
    }) as typeof window.open;
  });
}

test.describe('desktop pandas', () => {
  test('the sidebar header has no wordmark or mascot, and no startup intro plays', async ({ page }) => {
    await open(page);
    await expect(page.locator('html')).toHaveAttribute('data-ready', '');
    await expect(brand(page)).toHaveCount(0);
    await expect(page.locator('.brand-name, .desktop-brand, .desktop-brand-launch')).toHaveCount(0);
    // The header keeps its controls and stays a drag region.
    await expect(page.locator('.sidebar>.brand')).toHaveAttribute('data-tauri-drag-region', '');
    await expect(page.locator('.sidebar>.brand #view-menu')).toBeVisible();
    await expect(page.locator('.sidebar>.brand')).toHaveText('');
  });

  test('the rail foot holds only Remote access, Pull requests and Settings, with no logo; the 58px rail keeps them', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page);
    const names = () => page.locator('.sidebar-bottom > *').evaluateAll(items => items.map(item => item.getAttribute('aria-label')));
    expect(await names()).toEqual(['Remote access', 'Pull requests and issues', 'Settings']);
    await expect(page.locator('.project-rail .sidebar-bottom')).toHaveCount(1);
    await expect(page.locator('.star-panda, #theme-toggle, #support-button, #agent-usage-button, .sidebar-bots-bar')).toHaveCount(0);
    await page.setViewportSize({ width: 620, height: 940 });
    await expect.poll(() => page.locator('.sidebar').evaluate(node => node.getBoundingClientRect().width), { timeout: 15_000 }).toBe(58);
    expect(await names()).toEqual(['Remote access', 'Pull requests and issues', 'Settings']);
    await expect(page.locator('.sidebar > .sidebar-bottom')).toBeVisible();
  });

  test('what left the rail is a command in Go to anything: bots, help, usage, the theme and the star', async ({ page }) => {
    await recordWindowOpens(page);
    await open(page);
    await page.keyboard.press('ControlOrMeta+k');
    const query = page.getByRole('textbox', { name: 'Search tasks and commands', exact: true });
    const results = page.locator('#command-results').getByRole('button');
    for (const [typed, name] of [['bots in', /^Bots in the sidebar/], ['help', /^Help & support/], ['usage', /^Agent usage/], ['theme', /^Switch to light mode/], ['star', /^Star PandaCode on GitHub/]] as const) {
      await query.fill(typed);
      await expect(results.filter({ hasText: name })).toHaveCount(1);
    }
    // The star opens the canonical repository, once, only on an explicit pick.
    expect(await page.evaluate(() => (window as any).__externalUrlOpens)).toEqual([]);
    await results.filter({ hasText: /^Star PandaCode on GitHub/ }).click();
    await expect.poll(() => page.evaluate(() => (window as any).__externalUrlOpens)).toEqual([repository]);
    // The theme command flips the theme and renames itself.
    await runCommand(page, /^Switch to light mode/);
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await runCommand(page, /^Switch to dark mode/);
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  });
});
