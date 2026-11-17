import { expect, test, type Page } from '@playwright/test';
import { openPlus, pickFromPlus, plus, plusMenu } from './composer-more-helper';

// The composer's control row holds only `+`, the access mode, the model, the mic and Send. Team,
// Token saver, Council, the computer, the checkout, the branch and a linked PR are parked (in the
// DOM, invisible, drawn by their own modules) and reached from the `+` menu (src/composer-more.ts).
const tray = (page: Page) => page.locator('#composer-area .composer-tray');
const mode = (page: Page) => page.locator('#mode-picker-trigger');
const saver = (page: Page) => page.locator('.composer-saver-button');
const team = (page: Page) => page.locator('#team-trigger');
const council = (page: Page) => page.locator('.composer-council-button');
const computer = (page: Page) => page.locator('#composer-computer');
const branch = (page: Page) => page.locator('#branch-chip');
const iconColor = (page: Page) => mode(page).locator('.icon').first().evaluate(el => getComputedStyle(el).color);
const popped = (page: Page) => page.evaluate(() => (window as unknown as { trayLog: TrayLog }).trayLog.popped);
const item = (page: Page, name: string | RegExp) => plusMenu(page).getByRole('menuitem', { name, ...(typeof name === 'string' ? { exact: true } : {}) });
/** What the row shows, left to right: each visible control by its action or class. */
const rowControls = (page: Page) => page.locator('#composer').evaluate(form => [...form.querySelectorAll<HTMLElement>('.composer-actions>*,.composer-tools>*,.composer-parked *')]
  .filter(el => el.offsetWidth && getComputedStyle(el).visibility !== 'hidden')
  .sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left)
  .map(el => el.dataset.action ?? (el.matches('.composer-pill') ? 'send' : el.dataset.voice ? 'mic' : el.className)));

test('the row holds only +, the access mode, the model, the mic and Send; the mode shows its name', async ({ page }) => {
  await page.goto('/');
  await expect(tray(page)).toBeVisible();
  await expect(mode(page)).toBeVisible();
  await expect.poll(() => rowControls(page)).toEqual(['attach', 'mode', 'model', 'mic', 'send']);
  // The access mode reads as a word beside its icon.
  await expect(mode(page)).toContainText('Standard');
  expect((await mode(page).locator('.sr-only').boundingBox())!.width).toBeGreaterThan(30);
  await expect(mode(page)).toHaveAttribute('data-tip', /^Standard:/);
  await expect(mode(page)).toHaveAccessibleName('Mode: Standard');
  // `+` says it opens a menu; nothing else of the old row or the context row shows.
  await expect(plus(page)).toHaveAttribute('aria-haspopup', 'menu');
  for (const parked of [team(page), saver(page), council(page), branch(page), page.locator('#composer-area .composer-local')]) {
    await expect(parked).toHaveCount(1);
    await expect(parked).toBeHidden();
  }
  // One row: the card ends right under it.
  const [row, card] = await Promise.all([plus(page).boundingBox(), page.locator('#composer').boundingBox()]);
  expect(card!.y + card!.height - (row!.y + row!.height)).toBeLessThan(14);
});

test('the + menu lists what can be attached, then every moved control by its own name, icon and state', async ({ page }) => {
  await page.goto('/');
  await expect(tray(page)).toBeVisible();
  await expect(branch(page)).toHaveCount(1);
  const menu = await openPlus(page);
  await expect(plus(page)).toHaveAttribute('aria-expanded', 'true');
  const names = await menu.getByRole('menuitem').evaluateAll(rows => rows.map(row => row.getAttribute('aria-label')));
  expect(names.slice(0, 9)).toEqual(['Attach files…', 'Attach a folder…', 'Project files', 'Sketch a mockup…', 'Workflow recipes…', 'Schedule a wake-up…', 'Team: off', 'Token saver: off', 'Council: off']);
  // A started thread's computer is fixed, so only its checkout and branch follow.
  expect(names[9]).toMatch(/^Local: \//);
  expect(names[10]).toMatch(/^Branch \S+/);
  expect(names.length).toBe(11);
  // The moved controls start their own group, and each row leads with its control's icon.
  await expect(menu.getByRole('separator')).toHaveCount(1);
  const glyph = (name: string | RegExp) => item(page, name).locator('svg').first().innerHTML();
  const glyphs = [await glyph('Team: off'), await glyph('Token saver: off'), await glyph('Council: off'), await glyph(/^Local:/), await glyph(/^Branch /)];
  expect(new Set(glyphs).size).toBe(glyphs.length);
  expect(glyphs[0]).toBe(await team(page).locator('svg').first().innerHTML());
  // A control's own description rides along; the checkout is a note, not an action.
  await expect(item(page, 'Team: off')).toHaveAccessibleDescription(/Your model leads/);
  await expect(item(page, /^Branch /)).toHaveAccessibleDescription('Branch and worktree actions');
  await expect(item(page, /^Local:/)).toBeDisabled();
  for (const name of ['Team: off', 'Token saver: off', 'Council: off']) await expect(item(page, name)).toBeEnabled();
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(plus(page)).toBeFocused();
});

test('a control that is disabled is a disabled row', async ({ page }) => {
  await page.goto('/');
  await expect(team(page)).toHaveCount(1);
  await page.waitForFunction(() => document.documentElement.hasAttribute('data-ready'));
  // Disabled and opened in one step, before a background refresh can redraw the composer.
  await team(page).evaluate(button => { (button as HTMLButtonElement).disabled = true; document.querySelector<HTMLElement>('#composer-more')!.click(); });
  await expect(plusMenu(page)).toBeVisible();
  await expect(item(page, 'Team: off')).toBeDisabled();
  await expect(item(page, 'Council: off')).toBeEnabled();
  await expect(item(page, 'Token saver: off')).toBeEnabled();
});

test('with no branch to show, the + menu has no branch row', async ({ page }) => {
  await page.route('**/src/preview.ts*', async route => {
    const response = await route.fetch(), source = await response.text();
    await route.fulfill({ response, body: source.replace(/isRepo: true/, 'isRepo: false') });
  });
  await page.goto('/');
  await expect(tray(page)).toBeVisible();
  await expect(page.locator('#composer-area .composer-local')).toHaveCount(1);
  // The placeholder that holds the branch's place while git status loads is never a row.
  await expect(page.locator('#composer-area .branch-chip-pending')).toHaveCount(0);
  await expect(branch(page)).toHaveCount(0);
  await openPlus(page);
  await expect(item(page, /^Local:/)).toBeVisible();
  await expect(item(page, /^Branch/)).toHaveCount(0);
});

test('Team works from the + menu: its menu opens from +, a pick turns it on, and the row says so', async ({ page }) => {
  await page.goto('/');
  await expect(team(page)).toHaveCount(1);
  await pickFromPlus(page, team(page));
  const menu = page.getByRole('dialog', { name: 'Team', exact: true });
  await expect(menu.getByRole('menuitemradio', { name: 'No team' })).toBeVisible({ timeout: 10_000 });
  // It opens from `+`, which owns it: expanded while open, focused again after Esc.
  await expect(plusMenu(page)).toBeHidden();
  await expect(plus(page)).toHaveAttribute('aria-expanded', 'true');
  const [anchor, box] = await Promise.all([plus(page).boundingBox(), menu.boundingBox()]);
  expect(Math.abs(box!.x - anchor!.x)).toBeLessThan(2);
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(plus(page)).toBeFocused();
  await pickFromPlus(page, team(page));
  await menu.getByRole('menuitemradio', { name: 'Balanced team' }).click();
  await expect.poll(() => team(page).getAttribute('data-on')).toBe('');
  await expect(team(page)).toHaveAttribute('aria-busy', 'false');
  await openPlus(page);
  await expect(item(page, /^(?!Team: off).*: on$/)).toBeVisible();
  await expect(item(page, 'Team: off')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await pickFromPlus(page, team(page));
  await menu.getByRole('menuitemradio', { name: 'No team' }).click();
  await expect.poll(() => team(page).getAttribute('data-on')).toBe(null);
});

test('Token saver works from the + menu: its panel opens at +, and on is named in the row', async ({ page }) => {
  await page.goto('/');
  await expect(saver(page)).toHaveAttribute('aria-pressed', 'false');
  await pickFromPlus(page, saver(page));
  const panel = page.locator('.saver-panel');
  await expect(panel).toBeVisible();
  const [anchor, box] = await Promise.all([plus(page).boundingBox(), panel.boundingBox()]);
  // Placed from `+`: over it (centred, kept inside the window), right above it.
  const centre = anchor!.x + anchor!.width / 2;
  expect(box!.x).toBeLessThanOrEqual(centre);
  expect(box!.x + box!.width).toBeGreaterThanOrEqual(centre);
  expect(Math.abs(box!.x + box!.width / 2 - centre)).toBeLessThan(160);
  expect(Math.abs(box!.y + box!.height - anchor!.y)).toBeLessThan(16);
  await page.locator('.saver-switch').first().click();
  await expect(saver(page)).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
  await openPlus(page);
  await expect(item(page, 'Token saver: off')).toHaveCount(0);
  await expect(item(page, /^Token saver: /)).toBeVisible();
});

test('Council works from the + menu: its panel opens at + and turns Council on', async ({ page }) => {
  await page.goto('/');
  await expect(council(page)).toHaveCount(1);
  await pickFromPlus(page, council(page));
  const panel = page.locator('.council-panel');
  await expect(panel).toBeVisible();
  const [anchor, box] = await Promise.all([plus(page).boundingBox(), panel.boundingBox()]);
  const centre = anchor!.x + anchor!.width / 2;
  expect(box!.x).toBeLessThanOrEqual(centre);
  expect(box!.x + box!.width).toBeGreaterThanOrEqual(centre);
  expect(box!.y + box!.height).toBeLessThanOrEqual(anchor!.y + 1);
  await expect(panel.getByRole('switch', { name: 'Council' })).toBeFocused();
  await panel.getByRole('switch', { name: 'Council' }).click();
  await expect(council(page)).toHaveAttribute('data-on', '', { timeout: 10_000 });
  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
  await expect(plus(page)).toBeFocused();
  await openPlus(page);
  await expect(item(page, 'Council: off')).toHaveCount(0);
  await expect(item(page, /^Council/)).toBeVisible();
});

test('the computer (on a new thread) and the branch work from the + menu', async ({ page }) => {
  await page.goto('/');
  await expect(branch(page)).toHaveCount(1);
  // Only a thread that has not started chooses its computer.
  await expect(computer(page)).toHaveCount(0);
  await page.locator('.new-task-button').click();
  await expect(computer(page)).toHaveCount(1);
  await expect(computer(page)).toBeHidden();
  await pickFromPlus(page, computer(page));
  const runOn = page.getByRole('dialog', { name: 'Run on', exact: true });
  await expect(runOn.getByRole('menuitemradio').first()).toBeVisible();
  await expect(plus(page)).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('Escape');
  await expect(runOn).toBeHidden();
  await expect(plus(page)).toBeFocused();
  await pickFromPlus(page, branch(page));
  const actions = page.getByRole('dialog', { name: 'Branch and worktree', exact: true });
  await expect(actions.getByRole('menuitem').first()).toBeVisible();
  const [anchor, box] = await Promise.all([plus(page).boundingBox(), actions.boundingBox()]);
  expect(Math.abs(box!.x - anchor!.x)).toBeLessThan(2);
  await page.keyboard.press('Escape');
  await expect(plus(page)).toBeFocused();
});

test('a linked issue or pull request is a row of the + menu, and the row presses its chip', async ({ page }) => {
  await page.goto('/');
  await page.locator('#navigation .task-item', { hasText: 'Add keyboard shortcuts' }).click();
  const chip = page.locator('#composer-area .link-chip');
  await expect(chip).toHaveCount(1);
  await expect(chip).toBeHidden();
  const name = (await chip.getAttribute('aria-label'))!;
  expect(name).toMatch(/^(Pull request|Issue) \d+: /);
  // Its row presses the chip, so the link opens as it did (tests/github.spec.ts follows both kinds to where they open).
  // Listened for on the document: the context bar redraws its chips as git status arrives.
  await page.evaluate(() => document.addEventListener('click', event => { if ((event.target as Element).closest('.composer-parked .link-chip')) (window as unknown as { chipPressed?: boolean }).chipPressed = true; }, true));
  await pickFromPlus(page, chip);
  await expect.poll(() => page.evaluate(() => (window as unknown as { chipPressed?: boolean }).chipPressed)).toBe(true);
  await expect(plusMenu(page)).toBeHidden();
});

test('a parked control is never a Tab stop: Tab walks only the five controls of the row', async ({ page, browserName }) => {
  // WebKit on macOS skips buttons on a plain Tab (Safari's default); Alt+Tab walks every control.
  const tab = browserName === 'webkit' ? 'Alt+Tab' : 'Tab';
  await page.goto('/');
  await expect(mode(page)).toBeVisible();
  await page.locator('#prompt').focus();
  const stops: string[] = [];
  for (let step = 0; step < 5; step++) {
    await page.keyboard.press(tab);
    stops.push(await page.evaluate(() => { const el = document.activeElement as HTMLElement; return el.closest('.composer-parked') ? 'parked' : el.dataset.action ?? (el.dataset.voice ? 'mic' : el.matches('.send-button') ? 'send' : el.tagName); }));
  }
  expect(stops).not.toContain('parked');
  expect([...stops].sort()).toEqual(['attach', 'mic', 'mode', 'model', 'send']);
});

test('each mode tones its icon and menu row, and only a real change pops', async ({ page }) => {
  await page.goto('/');
  await expect(mode(page)).toBeVisible();
  await page.waitForFunction(() => document.documentElement.hasAttribute('data-ready'));
  const neutral = await iconColor(page);
  const colors = new Set([neutral]);
  for (const name of ['Accept edits', 'Plan mode', 'Full access']) {
    await mode(page).click();
    const row = page.getByRole('menuitemradio', { name });
    await expect(row.locator('svg').first()).toHaveAttribute('data-state-tone', /edit|plan|full/);
    await record(page, '#mode-picker-trigger');
    await row.click();
    await expect(mode(page)).toContainText(name);
    await expect.poll(() => popped(page)).toBe(true);
    await expect.poll(async () => { await page.waitForTimeout(250); return iconColor(page); }).not.toBe(neutral);
    colors.add(await iconColor(page));
  }
  expect(colors.size).toBe(4);
  // A redraw of the same state plays nothing: a thread round trip redraws the composer.
  await page.waitForTimeout(250);
  await record(page, '#mode-picker-trigger');
  await page.locator('#navigation .task-item', { hasText: 'Add keyboard shortcuts' }).click();
  await page.waitForTimeout(400);
  expect(await popped(page)).toBe(false);
});

for (const width of [1200, 829, 640, 520, 420]) test(`the controls fit one row, with no second row, in a ${width}px composer`, async ({ page }) => {
  await page.goto('/');
  await page.locator('#navigation .task-item', { hasText: 'Add keyboard shortcuts' }).click();
  await expect(page.locator('#composer-area .link-chip')).toHaveCount(1);
  await page.locator('#composer-area').evaluate((el, width) => { el.style.width = `${width}px`; el.style.maxWidth = `${width}px`; }, width);
  await page.waitForTimeout(100);
  // The card is one wrapping row (the tray and its groups are `display: contents`): `+`, the mode, the model,
  // the mic and Send share a line, and nothing follows it.
  const fit = await page.locator('#composer').evaluate(form => {
    const edge = form.getBoundingClientRect().right;
    const shown = (selector: string) => [...form.querySelectorAll<HTMLElement>(selector)].filter(item => item.offsetWidth && getComputedStyle(item).visibility !== 'hidden');
    const lines = (items: HTMLElement[]) => new Set(items.map(item => Math.round(item.getBoundingClientRect().top + item.offsetHeight / 2))).size;
    const controls = shown('.composer-actions>*,.composer-tools>*'), last = Math.max(...controls.map(item => item.getBoundingClientRect().bottom));
    return { overflow: form.scrollWidth - form.clientWidth, past: controls.filter(item => item.getBoundingClientRect().right > edge + 1).length, controls: controls.length, lines: lines(controls), below: shown('.composer-parked *').length, tail: form.getBoundingClientRect().bottom - last < 14 };
  });
  expect(fit).toEqual({ overflow: 0, past: 0, controls: 5, lines: 1, below: 0, tail: true });
  // The parked controls sit on `+` at every width, so what they open is placed from it.
  const [anchor, parked] = await Promise.all([plus(page).boundingBox(), page.locator('#branch-chip').evaluate(el => { const box = el.getBoundingClientRect(); return { x: box.left, y: box.top }; })]);
  expect(Math.max(Math.abs(parked.x - anchor!.x), Math.abs(parked.y - anchor!.y))).toBeLessThan(6);
});

test('the mode gives one tip, state first, on hover and on keyboard focus', async ({ page, browserName }) => {
  const tab = browserName === 'webkit' ? 'Alt+Tab' : 'Tab';
  await page.goto('/');
  await expect(mode(page)).toBeVisible();
  await expect(mode(page)).toHaveAttribute('aria-description', /\S/);
  await expect(mode(page)).not.toHaveAttribute('title', /./);
  await mode(page).hover();
  await expect(page.locator('.tip-bubble')).toHaveText(/^Standard:/);
  await page.mouse.move(0, 0);
  await mode(page).focus();
  await page.keyboard.press(`Shift+${tab}`);
  await page.keyboard.press(tab);
  await expect(mode(page)).toBeFocused();
  await expect(page.locator('.tip-bubble')).toHaveText(/^Standard:/);
});

/** Records whether a colour or fill transition started on `selector` (or inside it) and its icon popped, until read and reset.
 * Both are caught where they start (`transitionrun`, `animate()`), not by sampling frames: a starved frame
 * under load can skip a whole 180 ms animation or transition. */
type TrayLog = { faded: boolean; popped: boolean };
const record = (page: Page, selector: string) => page.evaluate(sel => {
  const w = window as unknown as { trayLog: TrayLog; trayHooked?: boolean; traySel: string };
  w.trayLog = { faded: false, popped: false }; w.traySel = sel;
  if (w.trayHooked) return;
  w.trayHooked = true;
  const animate = Element.prototype.animate;
  Element.prototype.animate = function (this: Element, ...args: Parameters<Element['animate']>) {
    if (this.matches('.icon') && this.closest(w.traySel) && JSON.stringify(args[0]).includes('scale')) w.trayLog.popped = true;
    return animate.apply(this, args);
  };
  document.addEventListener('transitionrun', event => { if (/^(background-)?color$/.test(event.propertyName) && (event.target as Element).closest?.(w.traySel)) w.trayLog.faded = true; }, true);
}, selector);
