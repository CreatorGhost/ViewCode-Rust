import { expect, test, type Page } from '@playwright/test';

// The rail's one usage ring (src/project-rail.ts): at the rail's foot, directly above Remote access, Pull
// requests and Settings. It shows the open thread's agent; with no thread open, the agent closest to its limit.

const rail = (page: Page) => page.locator('#project-rail');
const ring = (page: Page) => rail(page).locator('.rail-ring');
const dial = (page: Page) => ring(page).locator('.rail-ring-dial > svg');
const arc = (page: Page) => ring(page).locator('.rail-ring-arc');
const dialog = (page: Page) => page.getByRole('dialog', { name: 'Agent usage', exact: true });

/** Opens the preview with usage already remembered, as an earlier check would have left it. */
async function open(page: Page, peek: Record<string, number | null> = {}): Promise<void> {
  await page.addInitScript(values => {
    if (sessionStorage.getItem('usage-ring-seeded')) return;
    sessionStorage.setItem('usage-ring-seeded', '1');
    localStorage.setItem('pandacode.usage.peek', JSON.stringify(Object.fromEntries(Object.entries(values).map(([id, percent]) => [id, { percent, checkedAt: Date.now() }]))));
  }, peek);
  await page.goto('/');
  await expect(page.locator('#prompt')).toHaveAttribute('data-session-id', 'session-1');
  await expect(ring(page)).toHaveCount(1);
}
const openThread = async (page: Page, id: string) => {
  await page.locator(`.task-item[data-id="${id}"]`).click();
  await expect(page.locator('#prompt')).toHaveAttribute('data-session-id', id);
};
const colourOf = (page: Page, token: string) => rail(page).evaluate((node, name) => {
  const probe = document.createElement('i');
  probe.style.color = `var(${name})`;
  node.append(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color;
}, token);

test('there is one ring, at the foot of the rail directly above the three foot icons', async ({ page }) => {
  await open(page);
  await expect(rail(page).locator('.rail-ring')).toHaveCount(1);
  // The old per-agent group and its labels are gone.
  await expect(page.getByRole('group', { name: 'Usage by agent' })).toHaveCount(0);
  await expect(rail(page).locator('.rail-ring-label')).toHaveCount(0);
  const foot = rail(page).locator('.sidebar-bottom > button');
  await expect(foot).toHaveCount(3);
  expect(await foot.evaluateAll(buttons => buttons.map(button => button.getAttribute('aria-label')))).toEqual(['Remote access', 'Pull requests and issues', 'Settings']);
  const [box, first, tiles, host] = [await ring(page).boundingBox(), await foot.first().boundingBox(), await rail(page).locator('.rail-projects').boundingBox(), await rail(page).boundingBox()];
  // Directly above the icons, in their column, and nowhere near the middle of the rail.
  const gap = first!.y - (box!.y + box!.height);
  expect(gap).toBeGreaterThanOrEqual(0);
  expect(gap).toBeLessThanOrEqual(8);
  expect(Math.abs(box!.x + box!.width / 2 - (first!.x + first!.width / 2))).toBeLessThan(1);
  expect(box!.y).toBeGreaterThan(tiles!.y + tiles!.height);
  expect(box!.y).toBeGreaterThan(host!.y + host!.height * 0.7);
  expect(box!.width).toBeLessThanOrEqual(32);
  // It follows the foot icons in the tab order's document order: tiles, the ring, then the icons.
  expect(await rail(page).evaluate(node => [...node.querySelectorAll('button')].map(button => button.classList.contains('rail-tile') ? '' : button.getAttribute('aria-label')!.replace(/ usage: .*/, ' usage')).filter(Boolean))).toEqual(['Claude usage', 'Remote access', 'Pull requests and issues', 'Settings']);
});

test('the ring shows the open thread\'s agent, by name and figure, and "not checked yet" until a check', async ({ page }) => {
  await open(page, { claude: 41, codex: 12 });
  // session-1 runs Claude.
  await expect(ring(page)).toHaveAttribute('data-provider', 'claude');
  await expect(ring(page)).toHaveAccessibleName('Claude usage: 41% used');
  await expect(ring(page)).toHaveAttribute('title', 'Claude usage: 41% used');
  await expect(arc(page)).toHaveAttribute('stroke-dasharray', '41 100');
  await expect(dial(page)).not.toHaveAttribute('data-state-tone');
  await expect(ring(page).locator('.rail-ring-logo')).toHaveCount(1);
  // Another thread, another agent: the same ring renames itself. Codex is far from Claude's figure, so this is not "closest to its limit".
  await openThread(page, 'session-2');
  await expect(ring(page)).toHaveCount(1);
  await expect(ring(page)).toHaveAttribute('data-provider', 'codex');
  await expect(ring(page)).toHaveAccessibleName('Codex usage: 12% used');
  await expect(arc(page)).toHaveAttribute('stroke-dasharray', '12 100');
  // An agent nobody has checked: an empty track that says so.
  await openThread(page, 'session-3');
  await expect(ring(page)).toHaveAttribute('data-provider', 'kimi');
  await expect(ring(page)).toHaveAccessibleName(/^Kimi.* usage: not checked yet$/);
  await expect(ring(page)).toHaveAttribute('title', /usage: not checked yet$/);
  await expect(arc(page)).toHaveCount(0);
  await expect(ring(page).locator('.rail-ring-track')).toHaveCount(1);
  await expect(dial(page)).not.toHaveAttribute('data-state-tone');
});

test('near a limit the dial turns amber, and red once the limit is reached', async ({ page }) => {
  await open(page, { claude: 86, codex: 100, kimi: 79 });
  const stroke = () => arc(page).evaluate(node => getComputedStyle(node).stroke);
  // 86%: near. The tone is a state tone on the icon, never data-tone.
  await expect(ring(page)).toHaveAccessibleName('Claude usage: 86% used');
  await expect(dial(page)).toHaveAttribute('data-state-tone', 'near');
  expect(await stroke()).toBe(await colourOf(page, '--ds-warn'));
  expect(await ring(page).evaluate(node => node.querySelectorAll('[data-tone]').length + Number(node.hasAttribute('data-tone')))).toBe(0);
  // 100%: out.
  await openThread(page, 'session-2');
  await expect(ring(page)).toHaveAccessibleName('Codex usage: 100% used');
  await expect(dial(page)).toHaveAttribute('data-state-tone', 'limited');
  expect(await stroke()).toBe(await colourOf(page, '--ds-danger'));
  expect(await colourOf(page, '--ds-danger')).not.toBe(await colourOf(page, '--ds-warn'));
  // 79%: not yet.
  await openThread(page, 'session-3');
  await expect(ring(page)).toHaveAccessibleName(/usage: 79% used$/);
  await expect(dial(page)).not.toHaveAttribute('data-state-tone');
  expect(await stroke()).not.toBe(await colourOf(page, '--ds-warn'));
});

test('a click opens the Usage dialog with every agent, and its check updates the ring', async ({ page }) => {
  await open(page);
  await expect(ring(page)).toHaveAccessibleName('Claude usage: not checked yet');
  await ring(page).click();
  await expect(dialog(page)).toBeVisible();
  // Every agent is in the dialog, not only the ring's.
  const tabs = dialog(page).getByRole('tab');
  await expect(tabs.first()).toBeVisible();
  expect(await tabs.count()).toBeGreaterThan(3);
  for (const name of ['Overview', 'Claude', 'Codex', 'Kimi']) await expect(dialog(page).getByRole('tab', { name, exact: true })).toBeVisible();
  // The dialog's own check is remembered: the ring now carries Claude's figure.
  await expect(ring(page)).toHaveAccessibleName(/^Claude usage: \d+% used$/);
  await expect(arc(page)).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toBeHidden();
  // The keyboard opens it too.
  await ring(page).focus();
  await page.keyboard.press('Enter');
  await expect(dialog(page)).toBeVisible();
});
