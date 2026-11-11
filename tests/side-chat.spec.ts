// Side chats (docs/side-chat.md): a real session beside a working thread, streamed through the main
// transcript's own event path and renderer. The preview mocks the backend (`side_chat_*` in src/preview.ts).
import { expect, test, type Page } from '@playwright/test';

const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
const side = (page: Page) => page.getByRole('textbox', { name: 'Message the side thread', exact: true });
const log = (page: Page) => page.getByRole('log', { name: 'Side thread conversation', exact: true });
const liveText = (page: Page) => log(page).locator('.turn-card.is-live .live-text');
const calls = (page: Page, name: string): Promise<any[]> => page.evaluate(name => ((window as any).__sideCalls ?? []).filter((c: any) => c.command === name), name);
const mainId = (page: Page): Promise<string> => page.evaluate(() => (window as any).__mainId);
const emit = (page: Page, sessionId: string, kind: string, text: string) => page.evaluate(([sessionId, kind, text]) => (window as any).__previewEmit('agent-event', { sessionId, kind, text }), [sessionId, kind, text]);
const REPLY = 'The index costs **about 4 MB**.\n\n```ts\nconst index = createSearchIndex(items);\n```\n\n- built once\n- reused on every keystroke';

/** Asks in the side chat and returns its session id. Its run stays open (`__sideChatHold`) for the test to stream and end. */
async function ask(page: Page, question: string): Promise<string> {
  const before = (await calls(page, 'side_chat_send')).length;
  await prompt(page).fill(`/btw ${question}`);
  await prompt(page).press('Enter');
  await expect.poll(async () => (await calls(page, 'side_chat_send')).length).toBe(before + 1);
  return (await calls(page, 'side_chat_send')).at(-1).args.sessionId;
}
/** Streams `text` into a held side chat run and ends it, as the agent would. */
async function answer(page: Page, sideId: string, text = 'A short answer.'): Promise<void> {
  await emit(page, sideId, 'text', text);
  await page.evaluate(([id, text]) => (window as any).__sideChatFinish(id, text), [sideId, text]);
  await expect(log(page).locator('.turn-card:not(.is-live) .markdown').last()).toContainText(text.slice(0, 12));
  await expect(page.getByRole('button', { name: 'Send to side thread', exact: true })).toBeEnabled();
}
/** A bare `/btw` (the trailing space closes the slash picker) opens the panel on the thread's side chat. */
async function reopen(page: Page): Promise<void> {
  await prompt(page).fill('/btw ');
  await prompt(page).press('Enter');
  await expect(side(page)).toBeFocused();
}
/** The classes of every element under `selector`, in order: what the renderer produced, whatever the text. */
const shape = (page: Page, selector: string) => page.locator(selector).first().evaluate(root => [root, ...root.querySelectorAll('*')]
  .filter(node => !node.closest('svg, .message-actions, .turn-card-more, time, .turn-activity-meta') && !node.matches('.btw-insert, .team-progress-slot'))
  .map(node => `${node.tagName.toLowerCase()}.${[...node.classList].filter(name => name !== 'is-live').sort().join('.')}`));

test.beforeEach(async ({ page }) => {
  await page.route('**/src/preview.ts*', async route => {
    const response = await route.fetch(), source = await response.text();
    const marker = /switch\s*\(command\)\s*\{/;
    expect(source).toMatch(marker);
    // The main thread's run is held open, so it is working for the whole test.
    await route.fulfill({ response, body: source.replace(marker, `(window.__sideCalls ??= []).push({ command, args: structuredClone(args) });
      if (command === 'send_message' && args.sessionId === window.__mainId) { session(args.sessionId).status = 'running'; (messages[args.sessionId] ??= []).push({ id: ++id, sessionId: args.sessionId, role: 'user', content: args.text, createdAt: Date.now() }); return; }
      switch (command) {`) });
  });
  await page.setViewportSize({ width: 1440, height: 620 });
  await page.goto('/');
  await expect(prompt(page)).toBeEnabled();
  await page.evaluate(() => { const w = window as any; w.__mainId = document.querySelector<HTMLTextAreaElement>('#prompt')!.dataset.sessionId; w.__sideChatHold = true; });
  await prompt(page).fill('Build the feature');
  await prompt(page).press('Enter');
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
});

test('a side chat opens beside a running turn and streams token by token, leaving the main chat alone', async ({ page }) => {
  const main = await mainId(page);
  // The main agent is mid-answer, and the reader has scrolled up in it and is typing.
  await emit(page, main, 'text', 'Working on the feature.\n\n' + 'One more step of the plan.\n\n'.repeat(30));
  await expect(page.locator('#stream-region .live-text')).toContainText('Working on the feature.');
  const sideId = await ask(page, 'what does the index cost?');
  expect(sideId).not.toBe(main);
  await expect(page.locator('#inspector-title')).toHaveText('By the way');
  await expect(page.locator('.btw-head small')).toHaveText('Working · main running');
  await prompt(page).fill('my next instruction');
  await prompt(page).evaluate((input: HTMLTextAreaElement) => input.setSelectionRange(3, 3));
  const scrolled = await page.locator('#transcript').evaluate(transcript => {
    transcript.scrollTop = Math.max(0, transcript.scrollHeight - transcript.clientHeight - 400);
    transcript.dispatchEvent(new Event('scroll'));
    return { top: transcript.scrollTop, room: transcript.scrollHeight - transcript.clientHeight };
  });
  expect(scrolled.room).toBeGreaterThan(400);

  // Tokens arrive one event at a time; each shows before the next, long before the run ends.
  const chunks = REPLY.match(/[\s\S]{1,18}/g)!;
  await emit(page, sideId, 'text', chunks[0]);
  await expect(liveText(page)).toHaveText(/^The index costs/);
  expect((await liveText(page).textContent())!.length).toBeLessThan(30);
  await emit(page, sideId, 'text', chunks[1]);
  await expect(liveText(page)).toContainText('about 4 MB');
  await expect(log(page)).not.toContainText('reused on every keystroke');
  await expect(page.getByRole('button', { name: 'Stop side thread', exact: true })).toBeVisible();
  // The main agent keeps streaming meanwhile, in its own transcript only.
  await emit(page, main, 'text', 'Main thread text after the side chat started.');
  for (const chunk of chunks.slice(2)) await emit(page, sideId, 'text', chunk);
  await expect(liveText(page)).toContainText('reused on every keystroke');
  await expect(page.locator('#stream-region .live-text')).toContainText('Main thread text after the side chat started.');
  await expect(log(page)).not.toContainText('Main thread text');
  await expect(page.locator('#stream-region')).not.toContainText('The index costs');

  // The live reply is the main transcript's streaming Markdown in the main transcript's turn card.
  await expect(liveText(page).locator('strong')).toHaveText('about 4 MB');
  await expect(liveText(page).locator('pre code')).toContainText('createSearchIndex');
  await expect(liveText(page).locator('li')).toHaveCount(2);
  expect(await liveText(page).evaluate(node => node.className)).toBe(await page.locator('#stream-region .turn-card.is-live .live-text').evaluate(node => node.className));

  // None of it moved the main chat: same scroll position, same focus, same caret and draft.
  expect(await page.locator('#transcript').evaluate(transcript => transcript.scrollTop)).toBe(scrolled.top);
  await expect(prompt(page)).toBeFocused();
  await expect(prompt(page)).toHaveValue('my next instruction');
  expect(await prompt(page).evaluate((input: HTMLTextAreaElement) => input.selectionStart)).toBe(3);

  // The run ends: the saved turn replaces the live one, and the main thread is still working.
  await page.evaluate(([id, text]) => (window as any).__sideChatFinish(id, text), [sideId, REPLY]);
  await expect(log(page).locator('.turn-card:not(.is-live) .markdown strong')).toHaveText('about 4 MB');
  await expect(log(page).locator('.turn-card.is-live')).toHaveCount(0);
  await expect(page.locator('.btw-head small')).toHaveText('Ready · main running');
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
  expect(await page.locator('#transcript').evaluate(transcript => transcript.scrollTop)).toBe(scrolled.top);
  await expect(prompt(page)).toBeFocused();
  // Nothing was queued for, steered into or stopped in the main thread, and nothing written to it.
  for (const command of ['queue_add', 'steer_task', 'stop_session']) expect(await calls(page, command)).toHaveLength(0);
  expect((await calls(page, 'send_message')).map(call => call.args.sessionId)).toEqual([main]);
});

test('a saved side chat turn is the same markup as the main transcript draws for that reply', async ({ page }) => {
  const main = await mainId(page);
  const sideId = await ask(page, 'what does the index cost?');
  await emit(page, sideId, 'text', REPLY);
  await page.evaluate(([id, text]) => (window as any).__sideChatFinish(id, text), [sideId, REPLY]);
  await emit(page, main, 'text', REPLY);
  await page.evaluate(([id, text]) => (window as any).__sideChatFinish(id, text), [main, REPLY]);
  const sideCard = '.btw-transcript .turn-card:not(.is-live)', mainCard = '#transcript .turn-card:not(.is-live):last-of-type';
  await expect(page.locator(sideCard).locator('pre code')).toContainText('createSearchIndex');
  await expect(page.locator(mainCard).locator('pre code')).toContainText('createSearchIndex');
  // Same renderer, so the same elements and classes: card, reply card, Markdown, code block, list.
  const sideShape = await shape(page, sideCard);
  expect(sideShape).toEqual(await shape(page, mainCard));
  for (const part of ['section.turn-card', 'div.ds-card-inner.turn-card-reply', 'article.after-trace.agent-message.message', 'div.markdown.message-content', 'strong', 'li', 'pre', 'code']) expect(sideShape.some(name => name === part || name.startsWith(part + '.'))).toBe(true);
  // At panel width nothing runs past the panel's edge (the code block scrolls inside itself).
  const fits = await page.locator('.btw-transcript').evaluate(node => node.scrollWidth <= node.clientWidth + 1);
  expect(fits).toBe(true);
});

test('a side chat is hidden from the sidebar and its filter, and reopening the panel restores it', async ({ page }) => {
  const main = await mainId(page);
  const listed = await page.locator('.task-item').count();
  const sideId = await ask(page, 'zebra crossing question');
  await answer(page, sideId, 'Zebras cross here.');
  // It is a session of its own, linked to its thread and never listed.
  expect(await calls(page, 'side_chat_open')).toHaveLength(1);
  await expect(page.locator('.task-item')).toHaveCount(listed);
  await expect(page.locator(`.task-item[data-id="${sideId}"]`)).toHaveCount(0);
  await page.getByRole('searchbox', { name: 'Filter tasks', exact: true }).fill('zebra');
  await expect(page.locator('.task-item')).toHaveCount(0);
  await page.getByRole('searchbox', { name: 'Filter tasks', exact: true }).fill('');
  await expect(page.locator('.task-item')).toHaveCount(listed);

  // An unsent follow-up and a model pick belong to this side chat.
  await side(page).fill('half-typed follow-up');
  await page.locator('.btw-panel').getByRole('button', { name: /^Model: / }).click();
  await page.getByRole('dialog', { name: 'Harness and model', exact: true }).getByRole('option', { name: 'Claude Code · Claude Opus 5', exact: true }).click();
  await expect(page.locator('.btw-panel').getByRole('button', { name: /^Model: / })).toContainText('Claude Opus 5');
  expect((await calls(page, 'configure_session')).map(call => call.args.sessionId)).toEqual([sideId]);

  // Close the panel, visit another thread (whose panel is empty), come back: the same chat, text and model.
  await page.getByRole('button', { name: 'Close side thread panel', exact: true }).click();
  await expect(page.locator('.btw-panel')).toHaveCount(0);
  await page.locator(`.task-item:not([data-id="${main}"])`).first().click();
  await expect(prompt(page)).not.toHaveAttribute('data-session-id', main);
  await page.locator(`.task-item[data-id="${main}"]`).click();
  await expect(prompt(page)).toHaveAttribute('data-session-id', main);
  await reopen(page);
  await expect(page.locator('.btw-head strong')).toHaveText('zebra crossing question');
  await expect(log(page).locator('.user-message')).toHaveCount(1);
  await expect(log(page).locator('.turn-card .markdown')).toContainText('Zebras cross here.');
  await expect(side(page)).toHaveValue('half-typed follow-up');
  await expect(page.locator('.btw-panel').getByRole('button', { name: /^Model: / })).toContainText('Claude Opus 5');
  // Reopening read the thread's side chats again and opened no new one.
  expect((await calls(page, 'side_chat_list')).filter(call => call.args.parentId === main).length).toBeGreaterThan(1);
  expect(await calls(page, 'side_chat_open')).toHaveLength(1);
});

test('a side chat idle for 24 hours is gone, and the next question opens a new one', async ({ page }) => {
  const main = await mainId(page);
  const first = await ask(page, 'first question');
  await answer(page, first);
  // One minute short of a day it is still there when the panel reopens.
  await page.evaluate(() => { (window as any).__sideChatClock = 24 * 60 * 60 * 1000 - 60_000; });
  await page.getByRole('button', { name: 'Close side thread panel', exact: true }).click();
  await reopen(page);
  await expect(page.locator('.btw-head strong')).toHaveText('first question');
  // A day idle: the backend archives it and says so; the open panel empties without being touched.
  await page.evaluate(() => { (window as any).__sideChatClock = 24 * 60 * 60 * 1000 + 60_000; });
  await page.evaluate(parentId => (window as any).__previewEmit('side-chats-changed', { parentId, sessionId: '', reason: 'expired' }), main);
  await expect(page.getByText('No side thread yet')).toBeVisible();
  await expect(log(page)).toBeHidden();
  // It stays gone when the panel is reopened, and the next question starts a new chat.
  await page.getByRole('button', { name: 'Close By the way', exact: true }).click();
  await reopen(page);
  await expect(page.getByText('No side thread yet')).toBeVisible();
  await page.evaluate(() => { (window as any).__sideChatClock = 0; });
  const second = await ask(page, 'second question');
  expect(second).not.toBe(first);
  await expect(page.locator('.btw-head strong')).toHaveText('second question');
  await expect(log(page)).not.toContainText('first question');
  expect(await calls(page, 'side_chat_open')).toHaveLength(2);
});

test('the old /btw links in localStorage are dropped and never turn tasks into side chats', async ({ page }) => {
  const main = await mainId(page);
  const other = await page.locator(`.task-item:not([data-id="${main}"])`).first().getAttribute('data-id');
  await page.evaluate(([main, other]) => localStorage.setItem('pandacode.btw', JSON.stringify({ [main]: { sideId: other, seenId: 3 } })), [main, other]);
  await page.reload();
  await expect(prompt(page)).toBeEnabled();
  expect(await page.evaluate(() => localStorage.getItem('pandacode.btw'))).toBeNull();
  // The task the old link pointed at is still an ordinary, listed task.
  await expect(page.locator(`.task-item[data-id="${other}"]`)).toHaveCount(1);
  await reopen(page);
  await expect(page.getByText('No side thread yet')).toBeVisible();
});
