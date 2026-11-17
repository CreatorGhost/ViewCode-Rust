import test from 'node:test';
import assert from 'node:assert/strict';

// usage-peek.ts reads what was remembered from localStorage, once.
const now = Date.now();
const remembered = { codex: { percent: 41, checkedAt: now }, kimi: { percent: 93, checkedAt: now }, grok: { percent: 99, checkedAt: now - 25 * 60 * 60 * 1000 }, cline: { percent: 97, checkedAt: now }, pi: { percent: null, checkedAt: now } };
globalThis.localStorage = { getItem: key => key === 'pandacode.usage.peek' ? JSON.stringify(remembered) : null, setItem() {} };
const { usageRingAgent: railUsageAgent, usageTone, USAGE_NEAR } = await import('../src/usage-peek.ts');

const provider = (id, installed = true) => ({ id, name: id, installed, models: [] });
const providers = [provider('claude'), provider('codex'), provider('kimi'), provider('grok'), provider('cline', false), provider('pi')];

test('the ring shows the open thread\'s agent, whatever the others have used', () => {
  assert.equal(railUsageAgent(providers, 'claude', now), 'claude');
  assert.equal(railUsageAgent(providers, 'codex', now), 'codex');
  // Even one that is not installed here (a thread from another machine): its thread is what is open.
  assert.equal(railUsageAgent(providers, 'cline', now), 'cline');
});

test('with no thread open the ring shows the installed agent closest to its limit', () => {
  // Kimi at 93% beats Codex at 41%; Grok's 99% is a day old and says nothing; Cline's 97% is not installed.
  assert.equal(railUsageAgent(providers, undefined, now), 'kimi');
  assert.equal(railUsageAgent([provider('codex'), provider('claude')], undefined, now), 'codex');
  // Nothing checked yet: the first installed agent, as "not checked yet".
  assert.equal(railUsageAgent([provider('cline', false), provider('claude'), provider('pi')], undefined, now), 'claude');
  assert.equal(railUsageAgent([provider('cline', false)], undefined, now), undefined);
  assert.equal(railUsageAgent([], undefined, now), undefined);
});

test('a figure takes a state tone near the limit, and none below it or while unknown', () => {
  assert.equal(USAGE_NEAR, 80);
  assert.deepEqual([null, 0, 41, 79, 80, 93, 99, 100, 140].map(usageTone), ['', '', '', '', 'near', 'near', 'near', 'limited', 'limited']);
});
