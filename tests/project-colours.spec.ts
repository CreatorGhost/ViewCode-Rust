import { expect, test, type Page } from '@playwright/test';

// Project colours (src/project-colors.ts): a project's menu on its rail tile sets one of 18 colours; the tile and
// the rows' badge wear it, and the composer glow and the open row follow it while "Colour the app by the active
// project" is on.

const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
const composer = (page: Page) => page.locator('#composer');
const workspace = (page: Page) => page.locator('.workspace');
const tile = (page: Page, name: string) => page.locator('#rail-projects').getByRole('button', { name, exact: true });
const menu = (page: Page) => page.locator('.choice-menu');
const swatch = (page: Page, name: string) => menu(page).getByRole('menuitemradio', { name, exact: true });
const NAMES = ['Gray', 'Red', 'Orange', 'Amber', 'Yellow', 'Lime', 'Green', 'Emerald', 'Teal', 'Cyan', 'Sky', 'Blue', 'Indigo', 'Violet', 'Purple', 'Fuchsia', 'Pink', 'Rose'];

/** Runs stay open, as a long turn would, until Stop. */
async function holdRuns(page: Page): Promise<void> {
  await page.route('**/src/preview.ts*', async route => {
    const response = await route.fetch(), source = await response.text();
    const marker = /switch\s*\(command\)\s*\{/;
    expect(source).toMatch(marker);
    await route.fulfill({ response, body: source.replace(marker, `if (command === 'send_message') { session(args.sessionId).status = 'running'; return; }
      switch (command) {`) });
  });
}
async function open(page: Page): Promise<void> {
  await page.goto('/');
  await expect(prompt(page)).toBeEnabled();
}
async function startRun(page: Page): Promise<void> {
  await prompt(page).fill('Tidy the release notes');
  await prompt(page).press('Enter');
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
}
/** A colour as the browser resolves it on an element (a token or any CSS colour), as its channels. */
const resolved = (page: Page, value: string) => workspace(page).evaluate((node, colour) => {
  const probe = document.createElement('i');
  probe.style.color = colour;
  node.append(probe);
  const out = getComputedStyle(probe).color;
  probe.remove();
  return out;
}, value);
const channels = (color: string) => { const [r, g, b] = color.match(/[\d.]+/g)!.map(Number); return color.startsWith('color(') ? [r, g, b].map(value => Math.round(value * 255)) : [r, g, b]; };
const token = async (page: Page, name: string) => channels(await resolved(page, `var(${name})`));
/** The colours a computed box-shadow or background names, alpha dropped. */
const coloursOf = (value: string) => (value.match(/(rgba?|color)\([^)]+\)/g) ?? []).map(channels);
const tileTone = (page: Page, name: string) => tile(page, name).evaluate(node => (node as HTMLElement).style.getPropertyValue('--tile-tone'));
async function openSettingsAppearance(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('region', { name: 'Settings', exact: true }).getByRole('tab', { name: 'Appearance', exact: true }).click();
}

test('a project without a colour is neutral; its menu offers None and 18 named swatches', async ({ page }) => {
  await open(page);
  for (const name of ['orbit', 'pandacode']) {
    await expect(tile(page, name)).not.toHaveAttribute('data-color', /./);
    expect(await tileTone(page, name)).toBe('');
  }
  await expect(workspace(page)).not.toHaveAttribute('data-project-tint', /./);

  await tile(page, 'orbit').click({ button: 'right' });
  await expect(menu(page)).toBeVisible();
  await expect(menu(page)).toHaveAttribute('aria-label', 'orbit: project actions');
  const group = menu(page).getByRole('group', { name: 'Colour', exact: true });
  await expect(group.getByRole('menuitemradio')).toHaveCount(19);
  // Each swatch has a name (and the same tooltip), in ProjectColor order; None is the current one.
  expect(await group.getByRole('menuitemradio').evaluateAll(nodes => nodes.map(node => node.getAttribute('aria-label')))).toEqual(['None', ...NAMES]);
  expect(await group.getByRole('menuitemradio').evaluateAll(nodes => nodes.every(node => node.getAttribute('title') === node.getAttribute('aria-label')))).toBe(true);
  await expect(swatch(page, 'None')).toHaveAttribute('aria-checked', 'true');
  await expect(group.locator('[aria-checked=true]')).toHaveCount(1);
  // A swatch is filled with its tone token; the 18 are all different, and None is empty.
  const fills = await group.locator('.choice-menu-swatch-dot:not([data-empty])').evaluateAll(nodes => nodes.map(node => getComputedStyle(node).backgroundColor));
  expect(fills).toHaveLength(18);
  expect(new Set(fills).size).toBe(18);
  expect(channels(fills[13])).toEqual(await token(page, '--project-violet'));
  expect(await swatch(page, 'None').locator('.choice-menu-swatch-dot').evaluate(node => getComputedStyle(node).backgroundColor)).toBe('rgba(0, 0, 0, 0)');
  // The grid stays inside the menu, and the project's other actions are there too.
  const box = (await menu(page).boundingBox())!;
  for (const name of ['None', 'Teal', 'Rose']) { const dot = (await swatch(page, name).boundingBox())!; expect(dot.x).toBeGreaterThanOrEqual(box.x); expect(dot.x + dot.width).toBeLessThanOrEqual(box.x + box.width); expect(dot.width).toBeGreaterThanOrEqual(22); }
  await expect(menu(page).getByRole('menuitem', { name: 'New task', exact: true })).toBeVisible();
  await expect(menu(page).getByRole('menuitem', { name: 'Remove project…', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(menu(page)).toHaveCount(0);
  // Opening the menu did not choose the project.
  await expect(tile(page, 'orbit')).toHaveAttribute('aria-pressed', 'false');
});

test('picking a colour with the mouse paints the tile, the rows\' badges, the open row and the running glow', async ({ page }) => {
  await holdRuns(page);
  await open(page);
  const neutral = await tile(page, 'orbit').evaluate(node => getComputedStyle(node).backgroundColor);
  await tile(page, 'orbit').click({ button: 'right' });
  await swatch(page, 'Violet').click();
  await expect(menu(page)).toHaveCount(0);

  const violet = await token(page, '--project-violet'), accent = await token(page, '--accent');
  expect(violet).not.toEqual(accent);
  // The tile: its tone is the token, the fill and letter are mixes of it, the hairline is it.
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'violet');
  expect(await tileTone(page, 'orbit')).toBe('var(--project-violet)');
  const painted = await tile(page, 'orbit').evaluate(node => { const style = getComputedStyle(node); return { fill: style.backgroundColor, ink: style.color, edge: style.boxShadow, image: style.backgroundImage }; });
  expect(painted.fill).not.toBe(neutral);
  expect(painted.image).toBe('none');
  expect(coloursOf(painted.edge)[0]).toEqual(violet);
  // The letter reads on the fill (4.5:1), as tests/palette.test.mjs holds for every theme.
  const luminance = (c: number[]) => { const [r, g, b] = c.map(v => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
  const [ink, fill] = [luminance(channels(painted.ink)), luminance(channels(painted.fill))];
  expect((Math.max(ink, fill) + 0.05) / (Math.min(ink, fill) + 0.05)).toBeGreaterThanOrEqual(4.5);
  // The other project is untouched.
  await expect(tile(page, 'pandacode')).not.toHaveAttribute('data-color', /./);

  // The thread rows: orbit's badges wear the tone, pandacode's do not.
  const orbitBadges = page.locator('#navigation .task-app-icon[data-color="violet"]');
  await expect(orbitBadges).toHaveCount(3);
  expect(coloursOf(await orbitBadges.first().evaluate(node => getComputedStyle(node).boxShadow))[0]).toEqual(violet);
  await expect(page.locator('#navigation .task-app-icon:not([data-color])')).toHaveCount(1);

  // The open thread is orbit's: the app follows its colour (the open row), and a running turn glows in it.
  await expect(workspace(page)).toHaveAttribute('data-project-tint', 'violet');
  expect(channels(await resolved(page, 'var(--project-accent)'))).toEqual(violet);
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'violet');
  for (const layer of coloursOf(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).slice(0, 3)) expect(layer).toEqual(violet);

  // The menu now shows Violet as the current colour; None restores the neutral tile and the accent glow.
  await tile(page, 'orbit').click({ button: 'right' });
  await expect(swatch(page, 'Violet')).toHaveAttribute('aria-checked', 'true');
  await expect(swatch(page, 'None')).toHaveAttribute('aria-checked', 'false');
  await swatch(page, 'None').click();
  await expect(tile(page, 'orbit')).not.toHaveAttribute('data-color', /./);
  expect(await tile(page, 'orbit').evaluate(node => getComputedStyle(node).backgroundColor)).toBe(neutral);
  await expect(page.locator('#navigation .task-app-icon[data-color]')).toHaveCount(0);
  await expect(workspace(page)).not.toHaveAttribute('data-project-tint', /./);
  await expect(composer(page)).toHaveAttribute('data-glow', 'accent');
  for (const layer of coloursOf(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).slice(0, 3)) expect(layer).toEqual(accent);
});

test('the keyboard reaches the menu and every swatch; a second project keeps its own colour across a reload', async ({ page }) => {
  await open(page);
  // ⇧F10 on a focused tile opens its menu under it, on its first action.
  await tile(page, 'orbit').focus();
  await page.keyboard.press('Shift+F10');
  await expect(menu(page)).toBeVisible();
  await expect(menu(page).getByRole('menuitem', { name: 'New task', exact: true })).toBeFocused();
  // ↓ enters the swatches at the current one; → and ← move along them.
  await page.keyboard.press('ArrowDown');
  await expect(swatch(page, 'None')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(swatch(page, 'Gray')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(swatch(page, 'Red')).toBeFocused();
  await page.keyboard.press('ArrowLeft');
  await expect(swatch(page, 'Gray')).toBeFocused();
  // Typing a name jumps to it; Enter picks it and focus returns to the tile.
  await page.keyboard.type('tea');
  await expect(swatch(page, 'Teal')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(menu(page)).toHaveCount(0);
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'teal');
  await expect(tile(page, 'orbit')).toBeFocused();

  // The menu key on the other tile; ↓ from the swatches leaves the whole grid.
  await tile(page, 'pandacode').focus();
  await page.keyboard.press('ContextMenu');
  await expect(menu(page)).toHaveAttribute('aria-label', 'pandacode: project actions');
  // Teal is taken by orbit, and says so.
  await expect(menu(page).getByRole('menuitemradio', { name: 'Teal (used by another project)', exact: true })).toHaveCount(1);
  await page.keyboard.press('ArrowDown');
  await expect(swatch(page, 'None')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(menu(page).getByRole('menuitem', { name: /^(Reveal in Finder|Show in Explorer|Open folder)$/ })).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(swatch(page, 'None')).toBeFocused();
  await page.keyboard.type('am');
  await expect(swatch(page, 'Amber')).toBeFocused();
  await page.keyboard.press('Space');
  await expect(tile(page, 'pandacode')).toHaveAttribute('data-color', 'amber');
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'teal');
  expect(await token(page, '--project-amber')).not.toEqual(await token(page, '--project-teal'));

  // Both colours are stored with the project, not the page: a reload shows them again.
  await page.reload();
  await expect(prompt(page)).toBeEnabled();
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'teal');
  await expect(tile(page, 'pandacode')).toHaveAttribute('data-color', 'amber');
  await expect(page.locator('#navigation .task-app-icon[data-color="teal"]')).toHaveCount(3);
  await expect(page.locator('#navigation .task-app-icon[data-color="amber"]')).toHaveCount(1);
  // The app follows the open thread's project: orbit's teal, then pandacode's amber.
  await expect(workspace(page)).toHaveAttribute('data-project-tint', 'teal');
  await page.locator('#navigation').getByRole('button', { name: /Explore the project/ }).first().click();
  await expect(prompt(page)).toHaveAttribute('data-session-id', 'session-4');
  await expect(workspace(page)).toHaveAttribute('data-project-tint', 'amber');
  // Escape closes a menu without changing anything.
  await tile(page, 'pandacode').focus();
  await page.keyboard.press('Shift+F10');
  await expect(swatch(page, 'Amber')).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await expect(tile(page, 'pandacode')).toHaveAttribute('data-color', 'amber');
  await expect(tile(page, 'pandacode')).toBeFocused();
});

test('Settings → Appearance: with "Colour the app by the active project" off the glow and open row keep the accent; tiles and badges keep their colour', async ({ page }) => {
  await holdRuns(page);
  await open(page);
  // A colour arriving from elsewhere (`project-changed`) lands without a reload.
  await page.evaluate(() => (window as any).__previewProjectColor('project-1', 'rose'));
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'rose');
  await expect(workspace(page)).toHaveAttribute('data-project-tint', 'rose');
  const rowTinted = await page.locator('#navigation .task-item.selected').evaluate(node => getComputedStyle(node).backgroundColor);
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'rose');

  await openSettingsAppearance(page);
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  const tint = settings.getByRole('checkbox', { name: 'Colour the app by the active project', exact: true }), glow = settings.getByRole('checkbox', { name: 'Composer glow', exact: true });
  await expect(tint).toBeChecked();
  await expect(glow).toBeChecked();
  // The two sit together under one heading, the tint first.
  await expect(settings.getByRole('heading', { name: 'Project colour', exact: true })).toBeVisible();
  expect(await tint.evaluate((node, other) => node.closest('.settings-group') === (other as Element).closest('.settings-group') && Boolean(node.compareDocumentPosition(other as Node) & Node.DOCUMENT_POSITION_FOLLOWING), await glow.elementHandle())).toBe(true);

  await tint.uncheck();
  expect(await page.evaluate(() => localStorage.getItem('pandacode.projectTint'))).toBe('off');
  // The running thread's glow returns to the accent at once, and the open row to its neutral fill.
  const accent = await token(page, '--accent');
  await expect(composer(page)).toHaveAttribute('data-glow', 'accent');
  for (const layer of coloursOf(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).slice(0, 3)) expect(layer).toEqual(accent);
  await expect(workspace(page)).not.toHaveAttribute('data-project-tint', /./);
  expect(await workspace(page).evaluate(node => (node as HTMLElement).style.getPropertyValue('--project-accent'))).toBe('');
  // The project's own marks are its identity, not the app's accent: they stay.
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'rose');
  await expect(page.locator('#navigation .task-app-icon[data-color="rose"]').first()).toBeAttached();

  // The choice is kept across a reload, and so is the colour.
  await page.reload();
  await expect(prompt(page)).toBeEnabled();
  await expect(tile(page, 'orbit')).toHaveAttribute('data-color', 'rose');
  await expect(workspace(page)).not.toHaveAttribute('data-project-tint', /./);
  expect(await page.locator('#navigation .task-item.selected').evaluate(node => getComputedStyle(node).backgroundColor)).not.toBe(rowTinted);
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'accent');
  await openSettingsAppearance(page);
  await expect(page.getByRole('checkbox', { name: 'Colour the app by the active project', exact: true })).not.toBeChecked();
  await page.getByRole('checkbox', { name: 'Colour the app by the active project', exact: true }).check();
  await expect(composer(page)).toHaveAttribute('data-glow', 'rose');
  await expect(workspace(page)).toHaveAttribute('data-project-tint', 'rose');
});

test('forced colours and Increase Contrast keep coloured tiles, badges and swatches legible', async ({ page }) => {
  await open(page);
  await page.evaluate(() => (window as any).__previewProjectColor('project-2', 'lime'));
  await expect(tile(page, 'pandacode')).toHaveAttribute('data-color', 'lime');

  await page.emulateMedia({ contrast: 'more' });
  // The letter is the text ink and the hairline the solid tone.
  expect(channels(await tile(page, 'pandacode').evaluate(node => getComputedStyle(node).color))).toEqual(await token(page, '--text'));
  expect(await tile(page, 'pandacode').evaluate(node => getComputedStyle(node).boxShadow)).not.toMatch(/rgba\([^)]*, 0\.\d+\)/);
  const badge = page.locator('#navigation .task-app-default[data-color="lime"]').first();
  expect(channels(await badge.evaluate(node => getComputedStyle(node).color))).toEqual(await token(page, '--text'));

  await page.emulateMedia({ contrast: 'no-preference', forcedColors: 'active' });
  // System colours on the tile and badge; the swatches keep their own colour (it is what they say) under an outline.
  const system = async (name: string) => channels(await resolved(page, name));
  expect(channels(await tile(page, 'pandacode').evaluate(node => getComputedStyle(node).color))).toEqual(await system('ButtonText'));
  expect(channels(await tile(page, 'pandacode').evaluate(node => getComputedStyle(node).backgroundColor))).toEqual(await system('ButtonFace'));
  expect(channels(await badge.evaluate(node => getComputedStyle(node).color))).toEqual(await system('ButtonText'));
  await tile(page, 'pandacode').click({ button: 'right' });
  const dot = swatch(page, 'Lime').locator('.choice-menu-swatch-dot');
  expect(await dot.evaluate(node => { const style = getComputedStyle(node); return [style.forcedColorAdjust, style.outlineStyle]; })).toEqual(['none', 'solid']);
  await expect(swatch(page, 'Lime')).toHaveAttribute('aria-checked', 'true');
  expect(await swatch(page, 'Lime').evaluate(node => getComputedStyle(node).outlineStyle)).toBe('solid');
});
