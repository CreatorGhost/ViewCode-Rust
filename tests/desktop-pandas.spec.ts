import { expect, test, type Page } from '@playwright/test';

const repository = 'https://github.com/aicodeking/pandacode';
const starRequest = 'I am sad because people are not starring my repo.. If you like it can you star it, please?';
const brand = (page: Page) => page.getByRole('button', { name: 'Replay PandaCode animation', exact: true });
const star = (page: Page) => page.getByRole('button', { name: 'Star PandaCode on GitHub', exact: true });
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

  test('the 58px rail keeps its footer', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.setViewportSize({ width: 620, height: 940 });
    await open(page);
    await expect.poll(() => page.locator('.sidebar').evaluate(node => node.getBoundingClientRect().width), { timeout: 15_000 }).toBe(58);
    await expect(brand(page)).toHaveCount(0);
    await expect(page.locator('.sidebar-bottom > button')).toHaveCount(7);
  });

  // Each shape must render where its own SVG attributes put it: page CSS (a fill-box transform
  // origin, say) must never move a rotated eye or ear off the authored geometry.
  for (const [name, art] of [['star panda', '.star-panda-art']] as const) {
    test(`${name} eyes and ears render at their authored geometry`, async ({ page }) => {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.setViewportSize({ width: 1440, height: 940 });
      await open(page);
      await expect(page.locator(art).first()).toBeVisible();
      const drift = await page.locator(art).first().evaluate((svg: SVGSVGElement) => {
        const shapes = (root: Element) => [...root.querySelectorAll('ellipse, circle, path, rect')].filter(el => !el.closest('defs'));
        const units = svg.viewBox.baseVal.width;
        const measure = (root: SVGSVGElement) => {
          const frame = root.getBoundingClientRect();
          return shapes(root).map(el => {
            const r = el.getBoundingClientRect(), k = units / frame.width;
            return [(r.x - frame.x) * k, (r.y - frame.y) * k, r.width * k, r.height * k];
          });
        };
        const live = measure(svg);
        const clone = svg.cloneNode(true) as SVGSVGElement;
        for (const el of [clone, ...clone.querySelectorAll('*')]) { el.removeAttribute('class'); el.removeAttribute('style'); }
        clone.setAttribute('width', String(units * 10));
        clone.setAttribute('height', String(svg.viewBox.baseVal.height * 10));
        const holder = document.createElement('div');
        holder.style.cssText = 'position:fixed;left:0;top:0;pointer-events:none;opacity:0;contain:layout';
        holder.append(clone);
        document.body.append(holder);
        const authored = measure(clone);
        holder.remove();
        const named = shapes(svg).map(el => [...el.classList, el.closest('[data-part]')?.getAttribute('data-part'), el.parentElement?.getAttribute('class')].filter(Boolean).join(' '));
        return live.map((box, i) => ({ shape: `${i}:${named[i]}`, delta: Math.max(...box.map((v, j) => Math.abs(v - authored[i][j]))) }));
      });
      expect(drift.length).toBeGreaterThan(5);
      expect(drift.filter(d => d.delta > 0.25)).toEqual([]);
    });
  }

  test('star panda shares the exact hover and keyboard tooltip', async ({ page, browserName }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page);
    await expect(star(page)).toHaveAccessibleDescription(starRequest);
    await expect(star(page)).not.toHaveAttribute('title');
    await star(page).hover();
    const tip = page.getByRole('tooltip');
    await expect(tip).toHaveCount(1);
    await expect(tip).toHaveClass(/tip-bubble/);
    await expect(tip).toHaveText(starRequest);
    await page.mouse.move(700, 300);
    await expect(tip).toHaveCount(0);
    await page.getByRole('button', { name: 'Settings', exact: true }).focus();
    // WebKit's default Tab skips buttons; Option-Tab is its full keyboard navigation.
    await page.keyboard.press(browserName === 'webkit' ? 'Alt+Tab' : 'Tab');
    await expect(star(page)).toBeFocused();
    await expect(tip).toHaveCount(1);
    await expect(tip).toHaveText(starRequest);
    await page.keyboard.press('Escape');
    await expect(tip).toHaveCount(0);
  });

  test('star panda opens the canonical repo once only after an explicit click', async ({ page }) => {
    const repoRequests: string[] = [];
    await page.route(/^https:\/\/(?:api\.)?github\.com\//, async route => {
      repoRequests.push(route.request().url());
      await route.abort();
    });
    await recordWindowOpens(page);
    await open(page);
    await star(page).hover();
    await expect(page.getByRole('tooltip')).toHaveText(starRequest);
    expect(await page.evaluate(() => (window as any).__externalUrlOpens)).toEqual([]);
    expect(repoRequests).toEqual([]);
    await star(page).click();
    await expect.poll(() => page.evaluate(() => (window as any).__externalUrlOpens)).toEqual([repository]);
    expect(repoRequests).toEqual([]);
    await expect(star(page)).toBeEnabled();
    await expect(page).toHaveURL(/\/$/);
  });

  for (const width of [190, 235, 440, 58]) {
    test(`sidebar footer fits ${width}px with Help first and star beside Settings`, async ({ page }) => {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.setViewportSize({ width: width === 58 ? 620 : 1440, height: 940 });
      await page.addInitScript(width => localStorage.setItem('pandacode.sidebarWidth', String(width === 58 ? 235 : width)), width);
      await open(page);
      const sidebar = page.locator('.sidebar');
      await expect.poll(() => sidebar.evaluate(node => node.getBoundingClientRect().width)).toBe(width);
      const footer = page.locator('.sidebar-bottom');
      const buttons = footer.locator(':scope > button');
      // Help, Usage, Pull requests, Remote access, Settings, the star and the theme.
      await expect(buttons).toHaveCount(7);
      await expect(buttons.first()).toHaveAccessibleName('Help & support');
      await expect(footer.locator('[data-action="settings"] + button')).toHaveAccessibleName('Star PandaCode on GitHub');
      await expect(footer.locator('.star-panda + button')).toHaveAttribute('id', 'theme-toggle');
      // The footer is the foot of the project rail; in the 58px icon rail it returns to the sidebar.
      const bounds = (await (width === 58 ? sidebar : page.locator('#project-rail')).boundingBox())!;
      expect(await footer.evaluate(node => node.parentElement!.id)).toBe(width === 58 ? 'workspace-sidebar' : 'project-rail');
      const boxes = [];
      for (const button of await buttons.all()) {
        await expect(button).toBeVisible();
        await expect(button).toBeEnabled();
        const box = (await button.boundingBox())!;
        expect(box.width).toBeGreaterThanOrEqual(28);
        expect(box.height).toBeGreaterThanOrEqual(28);
        expect(box.x).toBeGreaterThanOrEqual(bounds.x);
        expect(box.x + box.width).toBeLessThanOrEqual(bounds.x + bounds.width);
        expect(box.y + box.height).toBeLessThanOrEqual(bounds.y + bounds.height);
        for (const previous of boxes) {
          const overlaps = box.x < previous.x + previous.width && box.x + box.width > previous.x
            && box.y < previous.y + previous.height && box.y + box.height > previous.y;
          expect(overlaps).toBe(false);
        }
        boxes.push(box);
      }
    });
  }
});
