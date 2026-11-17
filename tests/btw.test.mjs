import test from 'node:test';
import assert from 'node:assert/strict';
import { FRESH, activeSideChat, isSideChat, parseBtw, sideChatsOf, sidePreferences } from '../src/btw.ts';

const storage = (seed = {}) => {
  const values = new Map(Object.entries(seed)); let writes = 0, reads = 0;
  return {
    getItem: key => { reads++; return values.get(key) ?? null; },
    setItem: (key, value) => { writes++; values.set(key, value); },
    removeItem: key => { values.delete(key); },
    values, counts: () => ({ reads, writes }),
  };
};
const chat = (id, parentSessionId, updatedAt, more = {}) => ({ id, kind: 'side', parentSessionId, archived: false, updatedAt, ...more });

test('/btw is parsed only at the start of a prompt', () => {
  assert.equal(parseBtw('/btw what is X?'), 'what is X?');
  assert.equal(parseBtw('  /BTW  multi\nline '), 'multi\nline');
  assert.equal(parseBtw('/btw'), '');
  assert.equal(parseBtw('/btwx hi'), null);
  assert.equal(parseBtw('say /btw hi'), null);
});

test('a thread owns its live side chats, newest first, and nothing else is one', () => {
  const tasks = [
    chat('old', 'main', 10), chat('new', 'main', 30), chat('expired', 'main', 40, { archived: true }),
    chat('other', 'elsewhere', 50), { id: 'main', kind: 'thread', archived: false, updatedAt: 60 }, { id: 'legacy', archived: false, updatedAt: 70 },
    // A side chat opened as a full thread keeps its parent link but is an ordinary thread.
    { id: 'promoted', kind: 'thread', parentSessionId: 'main', archived: false, updatedAt: 80 },
  ];
  assert.deepEqual(sideChatsOf(tasks, 'main').map(task => task.id), ['new', 'old']);
  assert.deepEqual(sideChatsOf(tasks, 'nobody'), []);
  assert.deepEqual(tasks.filter(isSideChat).map(task => task.id), ['old', 'new', 'expired', 'other']);
});

test('the panel shows the remembered side chat while it lives, else the newest, and none after New', () => {
  const chats = sideChatsOf([chat('a', 'main', 10), chat('b', 'main', 20)], 'main');
  assert.equal(activeSideChat(chats, undefined).id, 'b');
  assert.equal(activeSideChat(chats, 'a').id, 'a');
  assert.equal(activeSideChat(chats, 'expired').id, 'b', 'a remembered chat that is gone falls back to the newest');
  assert.equal(activeSideChat(chats, FRESH), undefined);
  assert.equal(activeSideChat([], 'a'), undefined);
});

test('preferences are read once, written only on change, and the old /btw links are dropped', () => {
  const store = storage({ 'pandacode.btw': '{"main":{"sideId":"task-1","seenId":4}}', 'pandacode.sidechat.active': '{"main":"a","bad":4}' });
  const picks = sidePreferences(store);
  assert.equal(store.values.has('pandacode.btw'), false);
  assert.equal(picks.get('main'), 'a');
  assert.equal(picks.get('bad'), undefined);
  for (let frame = 0; frame < 100; frame++) picks.get('main');
  assert.deepEqual(store.counts(), { reads: 1, writes: 0 }, 'streaming frames never touch storage');
  picks.set('main', 'a');
  assert.equal(store.counts().writes, 0);
  picks.set('main', FRESH); picks.set('other', 'b'); picks.set('other', undefined);
  assert.equal(picks.get('main'), FRESH);
  assert.deepEqual(JSON.parse(store.values.get('pandacode.sidechat.active')), { main: FRESH });
  assert.deepEqual(sidePreferences(storage({ 'pandacode.sidechat.active': 'not json' })).get('main'), undefined);
  // Storage that throws (private mode) leaves the picks in memory.
  const broken = sidePreferences({ getItem: () => { throw new Error('denied'); }, setItem: () => { throw new Error('denied'); }, removeItem: () => { throw new Error('denied'); } });
  broken.set('main', 'a');
  assert.equal(broken.get('main'), 'a');
});
