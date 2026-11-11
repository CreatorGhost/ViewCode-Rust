import { expect, test, type Page } from '@playwright/test';

// ?sections adds preview threads for every section: running, waiting for approval, a failed run
// and a usage limit (Active); two unseen finished turns (Finished); a pinned task; a chat; a long
// worktree title with a PR; enough project tasks to pass the cap; and twelve settled tasks.
// The list ends in one fold: Finished rows past the cap and every settled thread wait behind "N more".
const section = (page: Page, name: string) => page.locator(`#navigation [data-thread-group="${name}"]`);
const row = (page: Page, id: string) => page.locator(`#navigation .task-item[data-id="${id}"]`);
/** The fold that ends the list: "N more" closed, "Show less" open. */
const toggle = (page: Page) => page.locator('#navigation > .more-fold');
const pager = (page: Page, count: number) => section(page, 'completed').getByRole('button', { name: `Show ${count} more`, exact: true });
const action = (page: Page, name: string) => page.getByRole('button', { name, exact: true });
const menu = (page: Page) => page.getByRole('dialog', { name: 'Task actions', exact: true });
const ids = (page: Page, name: string) => section(page, name).locator('.task-item').evaluateAll(rows => rows.map(r => (r as HTMLElement).dataset.id));

async function open(page: Page): Promise<void> {
  await page.goto('/?sections');
  await expect(page.locator('.task-item.selected')).toHaveCount(1);
}
/** Finished is capped per section; opening the fold lists every row. */
const showAllFinished = (page: Page) => toggle(page).click();

test.describe('sidebar sections', () => {
  test('lists pinned, then work that needs you, finished work and chats, each under a heading with a count', async ({ page }) => {
    await open(page);
    const active = page.getByRole('region', { name: 'Active tasks', exact: true });
    const review = page.getByRole('region', { name: 'Finished tasks', exact: true });
    await expect(active.locator('.section-heading')).toHaveText('Active4');
    // Latest start first: running, approval, the failed run, then the usage limit.
    expect(await ids(page, 'active')).toEqual(['sections-running', 'sections-approval', 'sections-failed', 'sections-limited']);
    // Everything not running or settled is Finished, newest first.
    await expect(review.locator('.section-heading')).toHaveText('Finished18');
    expect(await ids(page, 'review')).toEqual(expect.arrayContaining(['sections-review-1', 'sections-review-2', 'sections-chat', 'session-2']));
    await expect(row(page, 'sections-approval').locator('.task-indicator')).toHaveAttribute('aria-label', 'Needs approval');
    await expect(row(page, 'sections-approval').locator('.task-status')).toHaveText('Approval');
    await expect(row(page, 'sections-failed').locator('.task-indicator')).toHaveAttribute('aria-label', 'Failed');
    await expect(row(page, 'sections-limited').locator('.task-indicator')).toHaveAttribute('aria-label', /^Usage limit · /);
    await expect(row(page, 'sections-limited').locator('.task-status')).toHaveText(/^Limited · (?:[A-Z][a-z]{2} )?\d/);
    await expect(row(page, 'sections-running').locator('.task-runtime')).toBeVisible();
    await expect(row(page, 'sections-review-1')).toHaveClass(/\bunread\b/);
    await expect(row(page, 'sections-review-1').locator('.task-indicator.unread')).toHaveAttribute('aria-label', 'Unread');
    expect(await ids(page, 'pinned')).toEqual(['sections-pinned']);
    expect(await page.locator('#navigation > .task-group').evaluateAll(groups => groups.slice(0, 3).map(g => (g as HTMLElement).dataset.threadGroup))).toEqual(['pinned', 'active', 'review']);
    await expect(section(page, 'chats')).toHaveCount(0);
    await expect(section(page, 'all')).toHaveCount(0);
    // Two-line cards in Pinned, Active and Finished; cards are compact.
    const card = await row(page, 'sections-approval').boundingBox();
    expect(card!.height).toBeLessThanOrEqual(54);
    // Each thread appears once.
    for (const id of ['sections-running', 'sections-review-1', 'session-1', 'sections-chat']) await expect(row(page, id)).toHaveCount(1);
  });


  test('Finished follows the per-section cap; the rest wait behind the fold with the settled threads; Pinned is never capped', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('pandacode.sidebar.sort', JSON.stringify({ perProject: 3 })));
    await open(page);
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(3);
    // 15 Finished rows past the cap and 12 settled threads.
    await expect(toggle(page)).toHaveText('27 more');
    await expect(section(page, 'review').locator('.folder-more')).toHaveCount(0);
    // Its text starts where the section's heading does.
    const textLeft = (node: Element) => { const range = document.createRange(); range.selectNodeContents(node); return range.getBoundingClientRect().left; };
    const headingLeft = await section(page, 'review').locator('.group-heading').evaluate(textLeft);
    expect(Math.abs(await toggle(page).evaluate(textLeft) - headingLeft)).toBeLessThan(1);
    await toggle(page).click();
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(18);
    await expect(toggle(page)).toHaveText('Show less');
    await toggle(page).click();
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(3);
    await expect(section(page, 'pinned').locator('.folder-more')).toHaveCount(0);
  });

  test('a long title keeps its branch tail and PR visible on the row', async ({ page }) => {
    await open(page);
    await showAllFinished(page);
    const long = row(page, 'sections-long');
    await expect(long.locator('.task-link')).toHaveText('#482');
    await expect(long.locator('.task-worktree')).toHaveAttribute('aria-label', 'Worktree');
    await expect(long).toHaveAttribute('title', /pandacode\/sidebar-redesign-compact-rows/);
  });

  test('Active and Finished fold from their headings and stay folded after reload', async ({ page }) => {
    await open(page);
    const heading = section(page, 'review').locator('.section-heading');
    await expect(heading).toHaveAttribute('aria-expanded', 'true');
    await expect(heading.locator('.section-rule')).toHaveCount(1);
    await heading.click();
    await expect(heading).toHaveAttribute('aria-expanded', 'false');
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(0);
    await expect(heading).toHaveText('Finished18');
    await page.reload();
    await expect(heading).toHaveAttribute('aria-expanded', 'false');
    await expect(section(page, 'active').locator('.task-item')).toHaveCount(4);
    await heading.click();
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(6);
  });
  test('All projects draws no folders and no Settled group: Pinned, Active, Finished, then the fold', async ({ page }) => {
    await open(page);
    await expect(page.locator('#navigation .folder-heading')).toHaveCount(0);
    const groups = () => page.locator('#navigation > *').evaluateAll(items => items.map(item => (item as HTMLElement).dataset.threadGroup ?? item.className));
    expect(await groups()).toEqual(['pinned', 'active', 'review', 'folder-more more-fold']);
    await expect(page.getByRole('button', { name: /^Settled/ })).toHaveCount(0);
    // Open, the settled threads follow Finished in place and the fold still ends the list.
    await toggle(page).click();
    expect(await groups()).toEqual(['pinned', 'active', 'review', 'completed', 'folder-more more-fold']);
    await expect(section(page, 'review').locator('.task-item').first().locator('.task-app-icon')).toBeVisible();
  });


  test('the fold starts closed, opens in place, pages in more and stays open after reload', async ({ page }) => {
    await open(page);
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'false');
    // 12 Finished rows past the cap of 6 and 12 settled threads.
    await expect(toggle(page)).toHaveText('24 more');
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(6);
    await expect(section(page, 'completed')).toHaveCount(0);
    // It ends the list, right under the last row.
    const last = (await section(page, 'review').locator('.task-item').last().boundingBox())!, fold = (await toggle(page).boundingBox())!;
    expect(fold.y).toBeGreaterThanOrEqual(last.y + last.height - 1);
    expect(fold.y - (last.y + last.height)).toBeLessThan(12);
    await toggle(page).click();
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'true');
    await expect(toggle(page)).toHaveText('Show less');
    // In place: the row that was last keeps its position, and the rest follow it.
    expect((await row(page, (await ids(page, 'review'))[5]!).boundingBox())!.y).toBeCloseTo(last.y, 0);
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(18);
    expect((await ids(page, 'completed')).slice(0, 3)).toEqual(['sections-done-1', 'sections-done-2', 'sections-done-3']);
    await expect(section(page, 'completed').locator('.task-item')).toHaveCount(10);
    await pager(page, 2).click();
    await expect(section(page, 'completed').locator('.task-item')).toHaveCount(12);
    await expect(section(page, 'completed').locator('.folder-more')).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem('pandacode.sidebar.completedOpen'))).toBe('true');
    await page.reload();
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'true');
    await expect(section(page, 'review').locator('.task-item')).toHaveCount(18);
    await expect(section(page, 'completed').locator('.task-item')).toHaveCount(10);
    // Closing puts everything back behind it, and that is saved too.
    await toggle(page).click();
    await expect(toggle(page)).toHaveText('24 more');
    await expect(section(page, 'completed')).toHaveCount(0);
    await page.reload();
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'false');
  });

  test('row titles keep their width on hover: the actions cover the age instead', async ({ page }) => {
    await open(page);
    await toggle(page).click();
    const line = row(page, 'sections-done-1').locator('.task-line').first();
    const title = row(page, 'sections-done-1').locator('.task-title');
    await expect(line).toHaveCSS('padding-right', '0px');
    await expect(row(page, 'sections-done-1').locator('.task-age')).toBeVisible();
    const width = (await title.boundingBox())!.width;
    await row(page, 'sections-done-1').hover();
    await expect(action(page, 'Reopen: Add dark mode tokens')).toBeVisible();
    await expect(line).toHaveCSS('padding-right', '0px');
    await expect(row(page, 'sections-done-1').locator('.task-age')).toHaveCSS('visibility', 'hidden');
    expect((await title.boundingBox())!.width).toBeCloseTo(width, 0);
  });

  test('the row tick settles a finished thread, Undo brings it back, and the checked tick reopens it', async ({ page }) => {
    await open(page);
    await toggle(page).click();
    await row(page, 'sections-review-1').hover();
    await action(page, 'Settle: Fix the flaky filter test').click();
    await expect(section(page, 'completed').locator('.task-item').first()).toHaveAttribute('data-id', 'sections-review-1');
    await expect(section(page, 'review').locator('.section-count')).toHaveText('17');
    await expect(pager(page, 3)).toBeVisible();
    await page.locator('#toast').getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(section(page, 'completed').locator('[data-id="sections-review-1"]')).toHaveCount(0);
    await expect(pager(page, 2)).toBeVisible();
    // Undo restores the stage and the unread mark, so it is back in Finished, still unread.
    await expect(section(page, 'review').locator('[data-id="sections-review-1"]')).toHaveCount(1);
    await expect(row(page, 'sections-review-1')).toHaveClass(/\bunread\b/);
    await row(page, 'sections-review-2').hover();
    await action(page, 'Settle: Write the release notes').click();
    await expect(section(page, 'completed').locator('.task-item').first()).toHaveAttribute('data-id', 'sections-review-2');
    await action(page, 'Reopen: Write the release notes').click();
    await expect(section(page, 'completed').locator('[data-id="sections-review-2"]')).toHaveCount(0);
    // Reopened, it is finished work again.
    await expect(section(page, 'review').locator('[data-id="sections-review-2"]')).toHaveCount(1);
  });

  test('the open thread stays under the pointer when its own tick moves it to another section', async ({ page }) => {
    await open(page);
    await toggle(page).click();
    await row(page, 'sections-done-2').click();
    await expect(row(page, 'sections-done-2')).toHaveAttribute('aria-current', 'page');
    const before = await row(page, 'sections-done-2').boundingBox();
    await action(page, 'Reopen: Remove the old onboarding').click();
    await expect(action(page, 'Settle: Remove the old onboarding')).toBeVisible();
    expect(await ids(page, 'completed')).toContain('sections-done-2');
    expect((await row(page, 'sections-done-2').boundingBox())!.y).toBeCloseTo(before!.y, 0);
    // Opening another thread lets it move to Finished.
    await row(page, 'session-2').click();
    await expect(section(page, 'review').locator('.section-count')).toHaveText('19');
    expect(await ids(page, 'completed')).not.toContain('sections-done-2');
  });

  test('a finished thread stays in Finished when opened and left, is unread until opened, and leaves only when settled', async ({ page }) => {
    await open(page);
    await expect(page.getByRole('button', { name: 'Archived tasks', exact: true })).toHaveCount(0);
    await expect(row(page, 'sections-review-2')).toHaveClass(/\bunread\b/);
    await row(page, 'sections-review-2').click();
    await expect(row(page, 'sections-review-2')).toHaveAttribute('aria-current', 'page');
    await expect(row(page, 'sections-review-2')).not.toHaveClass(/\bunread\b/);
    expect(await ids(page, 'review')).toContain('sections-review-2');
    await row(page, 'session-2').click();
    await expect(row(page, 'session-2')).toHaveAttribute('aria-current', 'page');
    await expect(section(page, 'review').locator('[data-id="sections-review-2"]')).toHaveCount(1);
    await expect(row(page, 'sections-review-2')).not.toHaveClass(/\bunread\b/);
    // The other finished thread was never opened: still unread.
    await expect(row(page, 'sections-review-1')).toHaveClass(/\bunread\b/);
    await row(page, 'sections-review-2').hover();
    await action(page, 'Settle: Write the release notes').click();
    await expect(section(page, 'review').locator('[data-id="sections-review-2"]')).toHaveCount(0);
    await toggle(page).click();
    await expect(pager(page, 3)).toBeVisible();
    await expect(section(page, 'completed').locator('.task-item').first()).toHaveAttribute('data-id', 'sections-review-2');
  });

  test('deletes a thread from the row menu after confirming', async ({ page }) => {
    await open(page);
    await row(page, 'sections-review-1').hover();
    const more = action(page, 'Task actions: Fix the flaky filter test');
    await expect(more).toHaveAttribute('aria-haspopup', 'menu');
    await more.click();
    await menu(page).getByRole('menuitem', { name: 'Delete task…', exact: true }).click();
    const confirm = page.getByRole('dialog', { name: 'Delete task?', exact: true });
    await expect(confirm).toContainText('Fix the flaky filter test');
    await confirm.getByRole('button', { name: 'Delete task', exact: true }).click();
    await expect(confirm).toBeHidden();
    await expect(row(page, 'sections-review-1')).toHaveCount(0);
    await expect(section(page, 'review').locator('.section-count')).toHaveText('17');
  });

  test('the sidebar has no filter box: Search covers it, and / is an ordinary key', async ({ page }) => {
    await open(page);
    await expect(page.locator('#workspace-sidebar').getByRole('searchbox')).toHaveCount(0);
    await expect(page.locator('#workspace-sidebar .sidebar-search, #sidebar-filter')).toHaveCount(0);
    await row(page, 'session-2').focus();
    await page.keyboard.press('/');
    await expect(row(page, 'session-2')).toBeFocused();
    await page.locator('#workspace-sidebar').getByRole('button', { name: /^Search/ }).click();
    await expect(page.getByRole('textbox', { name: 'Search tasks and commands', exact: true })).toBeFocused();
  });

  test('a thread opened by shortcut holds its section; once it runs it leaves Settled for good', async ({ page }) => {
    await open(page);
    await page.locator('body').click({ position: { x: 900, y: 300 } }).catch(() => undefined);
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    // The seventh row (after Pinned, the four Active ones and the open session-1) is the next Finished thread.
    await page.keyboard.press('ControlOrMeta+7');
    await expect(row(page, 'sections-review-1')).toHaveAttribute('aria-current', 'page');
    expect(await ids(page, 'review')).toContain('sections-review-1');
    await toggle(page).click();
    await row(page, 'sections-done-3').click();
    await action(page, 'Reopen: Document the keyboard map').click();
    expect(await ids(page, 'completed')).toContain('sections-done-3');
    const prompt = page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
    await prompt.fill('Pick this back up');
    await prompt.press('Enter');
    await expect(section(page, 'active').locator('[data-id="sections-done-3"]')).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeHidden({ timeout: 15_000 });
    expect(await ids(page, 'completed')).not.toContain('sections-done-3');
  });

  test('the row menu is grouped, offers moves only where rows can move, and renames without opening', async ({ page }) => {
    await open(page);
    await row(page, 'sections-review-2').click({ button: 'right' });
    const items = menu(page).getByRole('menuitem');
    // Finished follows the sidebar sort, so its rows can move.
    await expect(menu(page).getByRole('menuitem', { name: 'Move up', exact: true })).toBeVisible();
    await expect(menu(page).getByRole('menuitem', { name: 'Copy task ID', exact: true })).toBeVisible();
    expect((await items.allTextContents()).map(text => text.trim())).toEqual(['Pin task', 'Move up', 'Move down', 'Open beside', 'Rename…', 'Schedule a wake-up…', 'Mark as read', 'Copy project path', 'Copy task ID', 'Settle', 'Delete task…']);
    await page.keyboard.press('Escape');
    // A worktree task offers its branch.
    await showAllFinished(page);
    await row(page, 'sections-long').click({ button: 'right' });
    await expect(menu(page).getByRole('menuitem', { name: 'New task from pandacode/sidebar-redesign-compact-rows…', exact: true })).toBeVisible();
    await expect(menu(page).getByRole('menuitem', { name: 'Copy branch name', exact: true })).toBeVisible();
    await expect(menu(page).getByRole('menuitem', { name: 'Move up', exact: true })).toBeVisible();
    await menu(page).getByRole('menuitem', { name: 'Rename…', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Rename task', exact: true });
    await dialog.getByRole('textbox', { name: 'Task name', exact: true }).fill('Compact rows');
    await dialog.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(row(page, 'sections-long').locator('.task-title')).toHaveText('Compact rows');
    await expect(row(page, 'sections-long')).not.toHaveAttribute('aria-current', 'page');
  });

  test('the open Settled thread stays listed past the first page', async ({ page }) => {
    await open(page);
    await toggle(page).click();
    await pager(page, 2).click();
    await row(page, 'sections-old-8').click();
    await expect(row(page, 'sections-old-8')).toHaveAttribute('aria-current', 'page');
    // Closing and reopening resets the page to 10 rows; the open thread is still there.
    await toggle(page).click();
    await toggle(page).click();
    await expect(row(page, 'sections-old-8')).toBeVisible();
    await expect(section(page, 'completed').locator('.task-item')).toHaveCount(11);
  });

  test('a focused row’s actions fade into the row’s own background', async ({ page }) => {
    await open(page);
    await row(page, 'session-2').focus();
    // The row's background eases in; read it once it has settled.
    await expect.poll(() => row(page, 'session-2').evaluate(item => item.getAnimations().length)).toBe(0);
    const [rowBg, fade] = await row(page, 'session-2').evaluate(item => [getComputedStyle(item).backgroundColor, getComputedStyle(item.parentElement!).getPropertyValue('--row-bg')]);
    expect(rowBg).not.toBe('rgba(0, 0, 0, 0)');
    const probe = await page.evaluate(value => { const el = document.createElement('div'); el.style.background = value; document.body.append(el); const color = getComputedStyle(el).backgroundColor; el.remove(); return color; }, fade.trim());
    expect(probe).toBe(rowBg);
  });

  test('the sort menu fits a 720px window, with tasks per section as one segmented row', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await open(page);
    await page.getByRole('button', { name: 'Sort and view', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Sort and view', exact: true });
    const perProject = dialog.getByRole('group', { name: 'Show per section', exact: true });
    await expect(perProject).toBeInViewport();
    expect(await dialog.locator('.choice-menu-list').evaluate(list => list.scrollHeight <= list.clientHeight)).toBe(true);
    await expect(perProject.getByRole('menuitemradio')).toHaveText(['3', '6', '10', '15']);
    await expect(perProject.getByRole('menuitemradio', { name: '6 tasks', exact: true })).toHaveAttribute('aria-checked', 'true');
    // ← and → move along the row; Enter picks.
    await perProject.getByRole('menuitemradio', { name: '6 tasks', exact: true }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(perProject.getByRole('menuitemradio', { name: '10 tasks', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(dialog).toBeHidden();
    expect(await page.evaluate(() => JSON.parse(localStorage.getItem('pandacode.sidebar.sort')!).perProject)).toBe(10);
  });

  test('the fold scrolls with the list, with no dock, and takes the arrow keys', async ({ page }) => {
    await page.route('**/src/preview.ts*', async route => {
      const response = await route.fetch();
      const body = (await response.text()).replace(/switch\s*\(command\)\s*\{/, `if (!window.__manyRunning) { window.__manyRunning = true; for (let i = 0; i < 30; i++) sessions.push({ ...sessions[0], id: 'many-running-' + i, title: 'Running task ' + i, status: 'running', taskStage: 'in_progress', lastPromptAt: Date.now() - i * 60000, pinned: false, sidebarOrder: null }); }
$&`);
      await route.fulfill({ response, body });
    });
    await page.setViewportSize({ width: 1280, height: 800 });
    await open(page);
    await expect(section(page, 'active').locator('.task-item')).toHaveCount(34);
    const nav = page.locator('#navigation');
    const inView = async () => {
      const [h, n] = await Promise.all([toggle(page).boundingBox(), nav.boundingBox()]);
      return h!.y >= n!.y - 1 && h!.y + h!.height <= n!.y + n!.height + 1;
    };
    // Nothing is docked: at the top of a long list the fold is out of sight, and it is the list's last row.
    await nav.evaluate(node => { node.scrollTop = 0; });
    expect(await inView()).toBe(false);
    await expect(toggle(page)).toHaveCSS('position', 'static');
    await nav.evaluate(node => { node.scrollTop = node.scrollHeight; });
    expect(await inView()).toBe(true);
    // → opens it and ← closes it, as on a heading; ↑ goes back into the list.
    await toggle(page).focus();
    await page.keyboard.press('ArrowRight');
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'true');
    await expect(toggle(page)).toBeFocused();
    // The settled rows are part of the one scrolling list: no panel of their own.
    await expect(section(page, 'completed').locator('.task-item')).toHaveCount(10);
    expect(await section(page, 'completed').locator('.task-list').evaluate(node => [getComputedStyle(node).overflowY, node.scrollHeight <= node.clientHeight + 1])).toEqual(['visible', true]);
    await page.keyboard.press('ArrowUp');
    await expect(pager(page, 2)).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(toggle(page)).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(toggle(page)).toHaveAttribute('aria-expanded', 'false');
    await page.keyboard.press('ArrowUp');
    expect(await page.evaluate(() => document.activeElement?.closest<HTMLElement>('[data-thread-group]')?.dataset.threadGroup)).toBe('review');
  });
});

test('every row keeps one indicator node: a state change fades and moves nothing', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('pandacode.sidebar.completedOpen', 'true'));
  await page.goto('/?sections');
  const unread = page.locator('#navigation .task-item[data-id="sections-review-1"]');
  await expect(unread.locator('.task-indicator.unread')).toHaveAttribute('aria-label', 'Unread');
  for (const rowLocator of await page.locator('#navigation .task-item').all()) await expect(rowLocator.locator('.task-indicator')).toHaveCount(1);

  // Mark as read: the same node loses its class and its colour fades; the age never moves.
  const age = () => unread.locator('.task-age').evaluate(el => { const box = el.getBoundingClientRect(), row = el.closest('.task-item')!.getBoundingClientRect(); return [box.left - row.left, box.top - row.top, box.width, row.height]; });
  const before = await age();
  await unread.locator('.task-indicator').evaluate(node => {
    (window as any).__indicator = node; (window as any).__faded = [];
    node.addEventListener('transitionrun', event => (window as any).__faded.push((event as TransitionEvent).propertyName));
  });
  await unread.hover();
  await page.locator('#task-row-sections-review-1 .task-more').click();
  await page.getByRole('menuitem', { name: 'Mark as read' }).click();
  await expect(unread).not.toHaveClass(/\bunread\b/);
  await page.mouse.move(0, 0);
  const after = await unread.locator('.task-indicator').evaluate(node => ({ same: node === (window as any).__indicator, faded: (window as any).__faded, cls: node.className, hidden: node.getAttribute('aria-hidden'), role: node.getAttribute('role') }));
  expect(after).toMatchObject({ same: true, cls: 'task-indicator', hidden: 'true', role: null });
  expect(after.faded).toContain('background-color');
  expect(await age()).toEqual(before);

  // Every state, on a card and on a one-line row: the slot's neighbours stay put, the paint changes.
  const states = ['running', 'attention', 'failed', 'limited', 'unread', ''];
  for (const selector of ['#navigation .task-item:not(.task-oneline)', '#navigation .more-group .task-item.task-oneline']) {
    const results = await page.locator(selector).first().evaluate((item, states) => {
      const node = item.querySelector<HTMLElement>('.task-indicator')!, next = node.nextElementSibling!;
      return states.map(state => {
        node.className = `task-indicator ${state}`.trim();
        document.documentElement.style.setProperty('--ds-ease-in', '0s'); document.documentElement.style.setProperty('--ds-ease-out', '0s');
        const self = getComputedStyle(node), mark = getComputedStyle(node, '::before'), box = next.getBoundingClientRect(), row = item.getBoundingClientRect();
        return { state, place: [box.left, box.top, row.height], slot: self.width, dot: self.backgroundColor, mark: mark.backgroundColor, ring: mark.boxShadow };
      });
    }, states);
    const clear = 'rgba(0, 0, 0, 0)';
    for (const result of results) {
      expect(result.place).toEqual(results[0].place);
      expect(result.slot).toBe('6px');
      // A card's plain indicator is clear; a one-line row leads with it, so a row with no state keeps a quiet dot.
      expect(result.dot === clear).toBe(selector.includes(':not') ? result.state !== 'unread' : !['unread', ''].includes(result.state));
      if (result.state === 'limited' && selector.includes(':not')) expect(result.ring).not.toContain(`${clear} 0px 0px 0px 2px inset`);
      else expect(result.mark === clear).toBe(result.state === 'unread' || result.state === '');
    }
    await page.evaluate(() => { document.documentElement.style.removeProperty('--ds-ease-in'); document.documentElement.style.removeProperty('--ds-ease-out'); });
  }
});

test('rows sit straight in their section with no inner panel, and the open thread is a quiet filled row', async ({ page }) => {
  await page.goto('/?sections');
  const row = page.locator('#navigation .task-item.selected').first();
  await expect(row).toBeVisible();
  const list = page.locator('#navigation .task-group > .task-list').first();
  expect(await list.evaluate(el => { const style = getComputedStyle(el); return [style.borderTopWidth, style.backgroundColor, style.boxShadow]; })).toEqual(['0px', 'rgba(0, 0, 0, 0)', 'none']);
  // Sections carry no card either, and the open row is a fill with no edge or shadow.
  const group = page.locator('#navigation .task-group').first();
  expect(await group.evaluate(el => { const style = getComputedStyle(el); return [style.borderTopWidth, style.backgroundColor, style.boxShadow]; })).toEqual(['0px', 'rgba(0, 0, 0, 0)', 'none']);
  expect(await row.evaluate(el => { const style = getComputedStyle(el); return [style.boxShadow, style.backgroundColor !== 'rgba(0, 0, 0, 0)']; })).toEqual(['none', true]);
});
