import { expect, test, type Page } from '@playwright/test';

// Code review screens (docs/code-review.md): the start dialog (Quick, Deep, Security) from the Changes tab, the
// command palette, the project's menu and a findings card; the findings card in a review thread; rating; live
// updates on `review-changed`; the re-review summary; and the project's review skill. `?review` seeds a finished
// Deep review, a Quick review in progress and a re-review (src/preview-review.ts).

const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
const row = (page: Page, title: string) => page.locator('#navigation [data-action="select-task"]', { hasText: title }).first();
const card = (page: Page) => page.locator('#transcript .review-card');
const dialog = (page: Page) => page.locator('#dialog');
const starts = (page: Page) => page.evaluate(() => (window as any).__reviewStarts as Record<string, any>[]);

async function open(page: Page, path = '/?review'): Promise<void> {
  await page.goto(path);
  await expect(prompt(page)).toBeEnabled();
}
async function openThread(page: Page, title: string): Promise<void> {
  await row(page, title).click();
  await expect(page.locator('.crumb-title')).toHaveText(title);
}
async function palette(page: Page, command: string): Promise<void> {
  await page.keyboard.press('ControlOrMeta+K');
  await page.keyboard.type(command);
  await page.locator('#command-results button', { hasText: command }).first().click();
}
async function startWith(page: Page, depth: 'Quick' | 'Deep' | 'Security', focus = ''): Promise<void> {
  await expect(dialog(page).locator('#review-start-form')).toBeVisible();
  await dialog(page).getByRole('group', { name: 'Review depth' }).getByRole('button', { name: depth, exact: true }).click();
  if (focus) await dialog(page).getByLabel('Focus').fill(focus);
  const label = depth === 'Security' ? 'Start security audit' : `Start ${depth.toLowerCase()} review`;
  await dialog(page).getByRole('button', { name: label, exact: true }).click();
  await expect(dialog(page)).not.toBeVisible();
}

test('a review starts at each depth from the Changes tab, the command palette and the project menu', async ({ page }) => {
  await open(page);
  await openThread(page, 'Make search feel instant');

  // Changes tab: Quick, the default depth, with a focus.
  await page.locator('#inspector-content .cv-review').click();
  await expect(dialog(page).getByRole('heading', { name: 'Review changes' })).toBeVisible();
  await expect(dialog(page).getByRole('group', { name: 'Review depth' }).getByRole('button', { name: 'Quick', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await startWith(page, 'Quick', 'the retry logic');
  await expect(page.locator('.crumb-title')).toHaveText('Review: local changes');
  await expect(card(page).locator('.review-card-status')).toHaveText('Reviewing…');
  await expect(card(page).locator('.review-card-title')).toContainText('Quick review');
  let calls = await starts(page);
  expect(calls.at(-1)).toMatchObject({ projectId: 'project-1', mode: 'quick', focus: 'the retry logic', provider: 'claude' });
  expect(calls.at(-1)!.sourceSessionId).toBe('session-1');

  // Command palette: Deep, uncommitted changes only.
  await openThread(page, 'Make search feel instant');
  await palette(page, 'Review my changes');
  await dialog(page).getByRole('group', { name: 'What to review' }).getByRole('button', { name: 'Uncommitted', exact: true }).click();
  await startWith(page, 'Deep');
  await expect(page.locator('.crumb-title')).toHaveText('Deep review: uncommitted changes');
  calls = await starts(page);
  expect(calls.at(-1)).toMatchObject({ mode: 'deep', scope: { kind: 'uncommitted' } });

  // The project's menu on its rail tile: Security.
  await page.locator('#rail-projects').getByRole('button', { name: 'orbit', exact: true }).click({ button: 'right' });
  await page.locator('.choice-menu').getByRole('menuitem', { name: 'Review changes…' }).click();
  await startWith(page, 'Security');
  await expect(page.locator('.crumb-title')).toHaveText('Security audit: local changes');
  await expect(card(page).locator('.review-card-mark')).toBeVisible();
  calls = await starts(page);
  expect(calls.at(-1)).toMatchObject({ mode: 'security', projectId: 'project-1' });
  expect(calls).toHaveLength(3);
});

test('Increase Contrast and forced colours keep the card outlined and its pressed rating visible', async ({ page }) => {
  await page.emulateMedia({ contrast: 'more' });
  await open(page);
  await openThread(page, 'Deep review: uncommitted changes');
  const border = await card(page).evaluate(node => getComputedStyle(node).borderTopColor);
  const text = await page.evaluate(() => { const probe = document.createElement('i'); probe.style.color = 'var(--border)'; document.body.append(probe); const value = getComputedStyle(probe).color; probe.remove(); return value; });
  expect(border).toBe(text);
  await page.emulateMedia({ contrast: 'no-preference', forcedColors: 'active' });
  const helpful = card(page).locator('.review-finding[data-key="finding:0"]').getByRole('button', { name: 'Helpful', exact: true });
  await helpful.click();
  await expect(helpful).toHaveAttribute('aria-pressed', 'true');
  const [fill, ink] = await helpful.evaluate(node => [getComputedStyle(node).backgroundColor, getComputedStyle(node).color]);
  expect(fill).not.toBe(ink);
  expect(fill).not.toBe('rgba(0, 0, 0, 0)');
});

test('a refused start says why in the dialog', async ({ page }) => {
  await open(page);
  await page.evaluate(() => { (window as any).__reviewFail = { review_start_local: 'The review’s project was not found.' }; });
  await openThread(page, 'Make search feel instant');
  await palette(page, 'Review my changes');
  await dialog(page).getByRole('button', { name: 'Start quick review', exact: true }).click();
  await expect(dialog(page).getByRole('alert')).toHaveText('The review’s project was not found.');
  await expect(dialog(page).getByRole('button', { name: 'Start quick review', exact: true })).toBeEnabled();
  await dialog(page).getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(await starts(page)).toHaveLength(0);
});

test('the findings card shows the verdict, scores, findings by severity, needs checking and every part of a finding', async ({ page }) => {
  await open(page);
  await openThread(page, 'Deep review: uncommitted changes');
  const findings = card(page);
  await expect(findings).toHaveAttribute('aria-label', 'Deep review findings');
  await expect(findings.locator('.review-card-status')).toHaveText('6 issues');
  await expect(findings.locator('.review-verdict')).toHaveText('Request changes');
  await expect(findings.getByRole('img', { name: 'Merge confidence 2 of 5' })).toBeVisible();
  await expect(findings.getByRole('img', { name: 'Review effort 3 of 5' })).toBeVisible();
  await expect(findings.locator('.review-group-head')).toHaveText(['Blocker1', 'Important2', 'Nit1', 'Question1', 'Needs checking1']);
  const blocker = findings.locator('.review-finding[data-key="finding:0"]');
  await expect(blocker.locator('.review-finding-title')).toHaveText('Search lowercases the query once per item');
  await expect(blocker.getByLabel('Confidence 92%')).toBeVisible();
  await expect(blocker.getByRole('button', { name: 'Open src/lib/search-index.ts:5-7' })).toBeVisible();
  // Detail and the suggested fix go through the shared Markdown renderer.
  await expect(blocker.locator('.review-detail strong')).toHaveText('10,000 times on every keystroke');
  await expect(blocker.locator('.review-fix pre code')).toContainText('const needle = query.toLowerCase();');
  await expect(blocker.locator('.review-rule')).toHaveText('RuleHot paths allocate nothing per item (CONTRIBUTING.md).');
  await expect(blocker.locator('.review-lesson')).toContainText('Search input is debounced before it queries');
  await expect(blocker.locator('.ds-chip')).toHaveText('broke 3× here');
  await expect(findings.locator('.review-finding[data-key="finding:1"] .ds-chip')).toHaveText('pre-existing');
  await expect(findings.locator('[data-group="checking"] .review-finding-title')).toHaveText('A missing title would throw');
  await expect(findings.locator('.review-finding[data-key="finding:4"]').getByRole('button', { name: 'Open README.md' })).toBeVisible();
  // The severity colour is on the leading icon only.
  const tones = await findings.locator('.review-sev-icon[data-severity="blocker"]').evaluate(node => [getComputedStyle(node).color, getComputedStyle(node.closest('.review-finding')!).backgroundColor]);
  expect(tones[0]).not.toBe(tones[1]);
});

test('clicking a finding’s location opens the file at its line', async ({ page }) => {
  await open(page);
  await openThread(page, 'Deep review: uncommitted changes');
  await card(page).getByRole('button', { name: 'Open src/components/search.ts:9' }).click();
  await expect(page.locator('#file-tabs [role="tab"][aria-selected="true"]')).toHaveText('search.ts');
  await expect(page.locator('#file-view .file-view-path')).toHaveText('src/components/search.ts');
  const marked = page.locator('#file-view .code-line[data-line-target]');
  await expect(marked).toHaveCount(1);
  await expect(marked.locator('.line-number')).toHaveText('9');
});

test('a rating shows at once, can be taken back, asks why when down, and survives a reload', async ({ page }) => {
  await open(page);
  await openThread(page, 'Deep review: uncommitted changes');
  const first = card(page).locator('.review-finding[data-key="finding:0"]');
  const helpful = first.getByRole('button', { name: 'Helpful', exact: true });
  const unhelpful = first.getByRole('button', { name: 'Not helpful', exact: true });
  await helpful.click();
  await expect(helpful).toHaveAttribute('aria-pressed', 'true');
  await expect(helpful).toBeFocused();
  await helpful.click();
  await expect(helpful).toHaveAttribute('aria-pressed', 'false');
  await unhelpful.click();
  await expect(unhelpful).toHaveAttribute('aria-pressed', 'true');
  await first.getByRole('group', { name: 'Why not?' }).getByRole('button', { name: 'Too minor' }).click();
  await expect(first.locator('.review-reason')).toContainText('Too minor · noted for this project');
  const second = card(page).locator('.review-finding[data-key="check:0"]');
  await second.getByRole('button', { name: 'Helpful', exact: true }).click();
  await expect(second.getByRole('button', { name: 'Helpful', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const ratings = await page.evaluate(() => (window as any).__reviewRatings);
  expect(ratings).toEqual([
    { key: 'finding:0', rating: 'up', reason: null }, { key: 'finding:0', rating: null, reason: null },
    { key: 'finding:0', rating: 'down', reason: null }, { key: 'finding:0', rating: 'down', reason: 'Too minor' },
    { key: 'check:0', rating: 'up', reason: null },
  ]);

  await page.reload();
  await expect(prompt(page)).toBeEnabled();
  await openThread(page, 'Deep review: uncommitted changes');
  await expect(card(page).locator('.review-finding[data-key="finding:0"]').getByRole('button', { name: 'Not helpful', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(card(page).locator('.review-finding[data-key="check:0"]').getByRole('button', { name: 'Helpful', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(card(page).locator('.review-finding[data-key="finding:1"]').getByRole('button', { name: 'Helpful', exact: true })).toHaveAttribute('aria-pressed', 'false');
});

test('a review in progress updates live when the reviewer sends findings', async ({ page }) => {
  await open(page);
  await openThread(page, 'Review: local changes on main');
  await expect(card(page).locator('.review-card-status')).toHaveText('Still reviewing · 2 findings so far');
  await expect(card(page).locator('.review-live-dot')).toBeVisible();
  await expect(card(page).locator('.review-finding')).toHaveCount(2);
  await page.evaluate(() => (window as any).__reviewSubmit('review-live'));
  await expect(card(page).locator('.review-card-status')).toHaveText('3 issues');
  await expect(card(page).locator('.review-live-dot')).toHaveCount(0);
  await expect(card(page).locator('.review-finding-title').last()).toHaveText('Should Escape clear the query?');
  await expect(card(page).getByRole('button', { name: 'Review again' })).toBeVisible();
});

test('a review started here fills in as findings arrive', async ({ page }) => {
  await open(page);
  await openThread(page, 'Make search feel instant');
  await palette(page, 'Review my changes');
  await startWith(page, 'Quick');
  await expect(card(page).locator('.review-card-status')).toHaveText('Reviewing…');
  const id = await page.evaluate(() => (window as any).__reviewStarts && document.querySelector<HTMLElement>('.review-card')!.dataset.reviewCard!);
  await page.evaluate(sessionId => (window as any).__reviewSubmit(sessionId, false), id);
  await expect(card(page).locator('.review-card-status')).toHaveText('Still reviewing · 1 finding so far');
  await page.evaluate(sessionId => (window as any).__reviewSubmit(sessionId, true), id);
  await expect(card(page).locator('.review-card-status')).toHaveText('2 issues');
});

test('a re-review says what was fixed, and Review again starts one from the findings', async ({ page }) => {
  await open(page);
  await openThread(page, 'Deep review: local changes on search-speed');
  await expect(card(page).locator('.review-rereview')).toHaveText('Re-review · 2 fixed · 1 still open');
  await openThread(page, 'Deep review: uncommitted changes');
  await card(page).getByRole('button', { name: 'Review again' }).click();
  await expect(dialog(page).getByRole('heading', { name: 'Review again' })).toBeVisible();
  // The re-review keeps the earlier depth and checks the earlier findings; what to review is not asked.
  await expect(dialog(page).getByRole('group', { name: 'Review depth' }).getByRole('button', { name: 'Deep', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(dialog(page).getByRole('group', { name: 'What to review' })).toBeHidden();
  await dialog(page).getByRole('button', { name: 'Start deep review', exact: true }).click();
  await expect(dialog(page)).not.toBeVisible();
  expect((await starts(page)).at(-1)).toMatchObject({ mode: 'deep', sourceSessionId: 'review-full', basedOn: { threadId: 'review-full', activityId: 'review-act-full' } });
  await expect(card(page).locator('.review-card-status')).toHaveText('Reviewing…');
  const id = await page.locator('.review-card').getAttribute('data-review-card');
  await page.evaluate(sessionId => (window as any).__reviewSubmit(sessionId, true), id);
  await expect(card(page).locator('.review-rereview')).toHaveText('Re-review · 3 fixed · 1 still open');
});

test('the review skill shows lessons, takes edits and learns', async ({ page }) => {
  await open(page);
  await page.locator('#rail-projects').getByRole('button', { name: 'orbit', exact: true }).click({ button: 'right' });
  await page.locator('.choice-menu').getByRole('menuitem', { name: 'Review skill…' }).click();
  const skill = dialog(page).locator('.review-skill');
  await expect(dialog(page).getByRole('heading', { name: 'Review skill · orbit' })).toBeVisible();
  await expect(skill.locator('.review-skill-status .ds-row-sub')).toHaveText(/^Learned .+ · 4 lessons$/);
  await expect(skill.locator('.review-skill-summary')).toContainText('search staying fast');
  await expect(skill.locator('.review-lesson')).toHaveCount(4);
  await expect(skill.locator('[data-lesson="shared-modules-use-named-exports"] .review-lesson-standing')).toHaveText('off after your ratings');
  await expect(skill.locator('[data-lesson="new-api-routes-have-an-integration-test-beside-them"] .review-lesson-standing')).toHaveText('1 helpful, 1 not');
  await expect(skill.locator('.ds-section-head .ds-row-meta')).toHaveText('3 of 4 on');

  // Switch a learned lesson off, add one of your own, remove it again.
  await skill.getByRole('switch', { name: 'Use “Search input is debounced before it queries”' }).click();
  await expect(skill.getByRole('switch', { name: 'Use “Search input is debounced before it queries”' })).not.toBeChecked();
  await expect(skill.locator('.ds-section-head .ds-row-meta')).toHaveText('2 of 4 on');
  await skill.getByLabel('A lesson of your own').fill('Feature flags default to off');
  await skill.getByLabel('A lesson of your own').press('Enter');
  await expect(skill.locator('.review-lesson').first()).toContainText('Feature flags default to off');
  await expect(skill.getByLabel('A lesson of your own')).toHaveValue('');
  await skill.getByRole('button', { name: 'Remove “Feature flags default to off”' }).click();
  await expect(skill.locator('.review-lesson')).toHaveCount(4);
  // Learned lessons are switched off, never removed.
  await expect(skill.getByRole('button', { name: 'Remove “Search input is debounced before it queries”' })).toHaveCount(0);

  // A rating of a finding that cited a lesson counts into it.
  await dialog(page).getByRole('button', { name: 'Close dialog' }).click();
  await openThread(page, 'Deep review: uncommitted changes');
  await card(page).locator('.review-finding[data-key="finding:0"]').getByRole('button', { name: 'Helpful', exact: true }).click();
  await card(page).getByRole('button', { name: 'Review skill' }).click();
  await expect(skill.locator(`[data-lesson="search-input-is-debounced-before-it-queries"] .review-lesson-standing`)).toHaveText('1 helpful, 0 not');
  await dialog(page).getByRole('button', { name: 'Close dialog' }).click();

  // pandacode has no skill yet: learning starts a thread, which hands the lessons over.
  await page.locator('#rail-projects').getByRole('button', { name: 'pandacode', exact: true }).click({ button: 'right' });
  await page.locator('.choice-menu').getByRole('menuitem', { name: 'Review skill…' }).click();
  await expect(skill.locator('.review-skill-status .ds-row-sub')).toHaveText(/^Not learned yet/);
  await skill.getByRole('button', { name: 'Learn now' }).click();
  await expect(skill.locator('.review-skill-status .ds-row-sub')).toHaveText('Learning from this project’s history now.');
  await expect(skill.getByRole('button', { name: 'Learn now' })).toBeDisabled();
  await page.evaluate(() => (window as any).__reviewSkillFinish('project-2'));
  await expect(skill.locator('.review-skill-status .ds-row-sub')).toHaveText(/^Learned .+ · 2 lessons$/);
  await expect(skill.locator('.review-lesson')).toHaveCount(2);
  await skill.getByRole('button', { name: 'How it was learned' }).click();
  await expect(dialog(page)).not.toBeVisible();
  await expect(page.locator('.crumb-title')).toHaveText('Learn: how pandacode is reviewed');
});

test('the review skill is in the command palette for the open project', async ({ page }) => {
  await open(page, '/');
  await palette(page, 'Review skill');
  await expect(dialog(page).getByRole('heading', { name: /^Review skill · / })).toBeVisible();
});
