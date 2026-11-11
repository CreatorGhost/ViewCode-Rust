import { expect, test, type Page } from '@playwright/test';

// The composer glow (src/composer-glow.ts, viewcode-shell.css): a light circling the composer's edge over a
// static glow while a turn runs in the open thread, in the project's colour, else the accent.

const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
const composer = (page: Page) => page.locator('#composer');
const ring = (page: Page) => page.locator('#composer > .composer-live-ring');
const light = (page: Page) => ring(page).locator('> span');

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
async function open(page: Page, query = ''): Promise<void> {
  await page.goto(`/${query}`);
  await expect(prompt(page)).toBeEnabled();
}
async function startRun(page: Page): Promise<void> {
  await prompt(page).fill('Tidy the release notes');
  await prompt(page).press('Enter');
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
}
/** A token as the browser resolves it on the composer, e.g. `rgb(45, 212, 191)`. */
const resolved = (page: Page, token: string) => composer(page).evaluate((node, name) => {
  const probe = document.createElement('i');
  probe.style.color = `var(${name})`;
  node.append(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color;
}, token);
/** The channels of a computed colour, whichever notation the browser serialises it in. */
const channels = (color: string) => { const [r, g, b] = color.match(/[\d.]+/g)!.map(Number); return color.startsWith('color(') ? [r, g, b].map(value => Math.round(value * 255)) : [r, g, b]; };
/** The colours of the composer's glow layers (its box-shadow), without their alpha. */
const glowColours = (page: Page) => composer(page).evaluate(node => getComputedStyle(node).boxShadow.match(/(rgba?|color)\([^)]+\)/g) ?? []);
/** The colour the light is drawn in: the tail stop of its gradient (the glow colour at 30%). */
const lightColour = async (page: Page) => channels((await light(page).evaluate(node => getComputedStyle(node).backgroundImage)).match(/(rgba?|color)\([^)]+\) 180deg/)![0]);

test('the glow is there only while a turn runs: a light circling over a static glow, gone when the turn ends', async ({ page }) => {
  await open(page, '?slowturn');
  await expect(ring(page)).toHaveCount(0);
  await expect(composer(page)).not.toHaveClass(/composer-live/);
  const idleShadow = await composer(page).evaluate(node => getComputedStyle(node).boxShadow);

  await startRun(page);
  await expect(composer(page)).toHaveClass(/composer-live/);
  await expect(ring(page)).toHaveCount(1);
  await expect(ring(page)).toHaveAttribute('aria-hidden', 'true');
  // The light: one conic square, turned on the compositor (`rotate`), 3.2 s a lap, for as long as the turn runs.
  expect(await light(page).evaluate(node => { const style = getComputedStyle(node); return [style.animationName, style.animationDuration, style.animationTimingFunction, style.animationIterationCount, style.backgroundImage.startsWith('conic-gradient')]; }))
    .toEqual(['composer-live-ring', '3.2s', 'linear', 'infinite', true]);
  expect(await page.evaluate(() => [...document.styleSheets].flatMap(sheet => { try { return [...sheet.cssRules]; } catch { return []; } })
    .filter((rule): rule is CSSKeyframesRule => rule instanceof CSSKeyframesRule && rule.name === 'composer-live-ring').map(rule => [...rule.cssRules].map(frame => (frame as CSSKeyframeRule).style.cssText).join(' '))))
    .toEqual(['rotate: 360deg;']);
  // It is a 2px band at the edge that takes no clicks, and the prompt under it still does.
  expect(await ring(page).evaluate(node => { const style = getComputedStyle(node), box = node.getBoundingClientRect(), host = node.parentElement!.getBoundingClientRect(); return [style.padding, style.pointerEvents, style.overflow, Math.round(box.width - host.width), Math.round(box.height - host.height)]; }))
    .toEqual(['2px', 'none', 'hidden', 0, 0]);
  await prompt(page).click();
  await expect(prompt(page)).toBeFocused();
  // The static glow: three layers in the glow colour, over the card's own shadow.
  const shadow = await composer(page).evaluate(node => getComputedStyle(node).boxShadow);
  expect(shadow).not.toBe(idleShadow);
  expect(shadow).toMatch(/0px 0px 18px -2px/);
  expect(shadow).toMatch(/0px 0px 48px -8px/);
  // A redraw mid-turn keeps the light where it was: each drawing starts from a shared clock, not from the top.
  expect(await light(page).evaluate(node => /^-\d+ms$/.test((node as HTMLElement).style.animationDelay))).toBe(true);

  // The turn ends by itself: no ring, no glow.
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeVisible({ timeout: 60_000 });
  await expect(ring(page)).toHaveCount(0);
  await expect(composer(page)).not.toHaveClass(/composer-live/);
  expect(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).toBe(idleShadow);
});

test('the glow is the accent for a project without a colour', async ({ page }) => {
  await holdRuns(page);
  await open(page);
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'accent');
  const accent = channels(await resolved(page, '--accent'));
  expect(channels(await resolved(page, '--composer-glow'))).toEqual(accent);
  const layers = await glowColours(page);
  expect(layers.length).toBeGreaterThanOrEqual(3);
  for (const layer of layers.slice(0, 3)) expect(channels(layer)).toEqual(accent);
  expect(await lightColour(page)).toEqual(accent);
});

test('the glow takes the project colour when the thread\'s project has one, and stays the accent in another project', async ({ page }) => {
  await holdRuns(page);
  await open(page, '?projectcolor=violet');
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'violet');
  const violet = channels(await resolved(page, '--project-violet')), accent = channels(await resolved(page, '--accent'));
  expect(violet).not.toEqual(accent);
  expect(channels(await resolved(page, '--composer-glow'))).toEqual(violet);
  for (const layer of (await glowColours(page)).slice(0, 3)) expect(channels(layer)).toEqual(violet);
  expect(await lightColour(page)).toEqual(violet);
  // Every colour name has a tone to glow in.
  for (const name of ['gray', 'red', 'orange', 'amber', 'yellow', 'lime', 'green', 'emerald', 'teal', 'cyan', 'sky', 'blue', 'indigo', 'violet', 'purple', 'fuchsia', 'pink', 'rose'])
    expect(await page.evaluate(token => getComputedStyle(document.documentElement).getPropertyValue(token).trim(), `--project-${name}`), name).toMatch(/^#[0-9a-f]{6}$/);

  // A thread of the other project (no colour) glows in the accent.
  await page.locator('#navigation').getByRole('button', { name: /Explore the project/ }).first().click();
  await expect(prompt(page)).toHaveAttribute('data-session-id', 'session-4');
  await startRun(page);
  await expect(composer(page)).toHaveAttribute('data-glow', 'accent');
  expect(channels(await resolved(page, '--composer-glow'))).toEqual(accent);
});

test('Settings → Appearance → Composer glow is on by default and turns the glow off', async ({ page }) => {
  await holdRuns(page);
  await open(page);
  await startRun(page);
  await expect(ring(page)).toHaveCount(1);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('region', { name: 'Settings', exact: true }).getByRole('tab', { name: 'Appearance', exact: true }).click();
  const glow = page.getByRole('checkbox', { name: 'Composer glow', exact: true });
  await expect(glow).toBeChecked();
  await glow.uncheck();
  expect(await page.evaluate(() => localStorage.getItem('pandacode.composerGlow'))).toBe('off');
  // The running thread loses it at once, light and halo both.
  await expect(ring(page)).toHaveCount(0);
  await expect(composer(page)).not.toHaveClass(/composer-live/);
  await expect(composer(page)).toHaveClass(/composer-running/);
  expect(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).not.toMatch(/0px 0px 18px -2px/);

  // The choice is kept, and a new turn does not glow.
  await page.reload();
  await expect(prompt(page)).toBeEnabled();
  await startRun(page);
  await expect(composer(page)).toHaveClass(/composer-running/);
  await expect(ring(page)).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('region', { name: 'Settings', exact: true }).getByRole('tab', { name: 'Appearance', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: 'Composer glow', exact: true })).not.toBeChecked();
  await page.getByRole('checkbox', { name: 'Composer glow', exact: true }).check();
  await expect(ring(page)).toHaveCount(1);
});

test('under reduced motion nothing circles: an even band over the same static glow', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await holdRuns(page);
  await open(page);
  await startRun(page);
  await expect(ring(page)).toHaveCount(1);
  expect(await light(page).evaluate(node => { const style = getComputedStyle(node); return [style.animationName, style.backgroundImage]; })).toEqual(['none', 'none']);
  expect(await light(page).evaluate(node => node.getAnimations().length)).toBe(0);
  expect(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).toMatch(/0px 0px 18px -2px/);
  // Stopping the turn removes it.
  await page.getByRole('button', { name: 'Stop task', exact: true }).click();
  await expect(ring(page)).toHaveCount(0);
});

test('Increase Contrast and forced colours get a plain edge: no halo, nothing moving', async ({ page }) => {
  await holdRuns(page);
  await page.emulateMedia({ contrast: 'more' });
  await open(page);
  await startRun(page);
  expect(await light(page).evaluate(node => getComputedStyle(node).animationName)).toBe('none');
  expect(await composer(page).evaluate(node => getComputedStyle(node).boxShadow)).toBe('none');
  await page.emulateMedia({ contrast: 'no-preference', forcedColors: 'active' });
  await expect(ring(page)).toBeHidden();
});
