import { expect, test, type Page } from '@playwright/test';

// Chats list in Finished like any other task. The sidebar has no New chat button: Go to anything starts one.
const chats = (page: Page) => page.getByRole('region', { name: 'Finished tasks', exact: true });

async function newChat(page: Page): Promise<void> {
  // New chat has no sidebar button: it starts from Go to anything.
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('button', { name: 'New chat (no project)' }).click();
  await expect(page.locator('#prompt')).toHaveAttribute('placeholder', 'Ask anything…');
  await expect(chats(page).locator('.task-item.selected')).toHaveAttribute('title', /New conversation/);
}

test.beforeEach(async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.task-item.selected')).toHaveCount(1);
});

test('the sidebar offers New thread and Search only; New chat is a command', async ({ page }) => {
  await expect(page.locator('#workspace-sidebar .sidebar-actions > button')).toHaveText([/^New thread/, /^Search/]);
  await expect(page.locator('#workspace-sidebar').getByRole('button', { name: /New chat/ })).toHaveCount(0);
  await page.locator('#workspace-sidebar').getByRole('button', { name: /^Search/ }).click();
  await page.getByRole('textbox', { name: 'Search tasks and commands', exact: true }).fill('new chat');
  await expect(page.locator('#command-results').getByRole('button')).toHaveText([/New chat \(no project\)/]);
  await page.keyboard.press('Enter');
  await expect(page.locator('#prompt')).toHaveAttribute('placeholder', 'Ask anything…');
  await expect(chats(page).locator('.task-item.selected')).toHaveAttribute('title', /New conversation/);
});

test('a new chat lists under Finished, not a project folder, without project affordances', async ({ page }) => {
  await newChat(page);
  await expect(page.locator('.folder-heading', { hasText: 'Chat' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'What’s on your mind?' })).toBeVisible();
  await expect(page.locator('#breadcrumb')).toHaveText('Chat');
  await expect(page.locator('#deploy-button')).toBeHidden();
  await expect(page.locator('#composer-area .composer-context')).toBeHidden();
  await expect(page.locator('#prompt')).toHaveAttribute('placeholder', 'Ask anything…');

  // Leaving the chat brings the project affordances back.
  await page.locator('#navigation .task-item[data-id="session-1"]').click();
  await expect(page.locator('#deploy-button')).toBeVisible();
});

test('deleting a chat removes it and its hidden project', async ({ page }) => {
  await newChat(page);
  const id = await chats(page).locator('.task-item.selected').getAttribute('data-id');
  await page.getByRole('button', { name: 'Task actions', exact: true }).click();
  await page.getByRole('dialog', { name: 'Task actions', exact: true }).getByRole('menuitem', { name: 'Delete task…', exact: true }).click();
  await page.getByRole('dialog', { name: 'Delete task?', exact: true }).getByRole('button', { name: 'Delete task', exact: true }).click();
  await expect(page.locator(`#navigation .task-item[data-id="${id}"]`)).toHaveCount(0);
  // The project filter lists real projects only.
  await page.locator('#project-filter').click();
  await expect(page.getByRole('menuitemradio', { name: /orbit/ })).toBeVisible();
  await expect(page.getByRole('menuitemradio', { name: /^Chat\b/ })).toHaveCount(0);
});

test('Go to anything starts a chat, and an empty chat can move to a project', async ({ page }) => {
  await page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+k`);
  await page.getByRole('button', { name: 'New chat (no project)' }).click();
  await expect(chats(page).locator('.task-item.selected')).toHaveCount(1);

  await page.getByRole('button', { name: 'Move to project…' }).click();
  await page.getByRole('option', { name: /orbit/ }).click();
  await expect(page.locator('#breadcrumb')).not.toHaveText('Chat');
  await expect(chats(page).locator('.task-item.selected')).toHaveCount(1);
});
