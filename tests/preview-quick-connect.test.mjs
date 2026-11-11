import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createPreviewQuickConnect } from '../src/preview-quick-connect.ts';

function fixture(hooks = {}) {
  const events = [];
  const mock = createPreviewQuickConnect((name, payload) => events.push([name, payload]), () => hooks);
  return { ...mock, events, hooks };
}

test('the preview mock answers every registered quick_connect command', () => {
  const lib = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
  const registered = [...lib.matchAll(/quick_connect::(quick_connect_\w+),/g)].map(match => match[1]);
  assert.equal(registered.length, 15);
  const args = { accountId: 'aaaa1111', enabled: true, mode: 'sameWifi', visible: true, deviceId: 'phone-1', scope: 'view' };
  for (const command of registered) {
    const { invoke } = fixture();
    try { invoke(command, args); } catch (error) { assert.doesNotMatch(String(error), /Unknown Quick connect command/, command); }
  }
  assert.throws(() => fixture().invoke('quick_connect_nope'), /Unknown Quick connect command/);
});

test('setup, pairing, revoking and removal move through the stages', () => {
  const { invoke, pair, events } = fixture();
  assert.equal(invoke('quick_connect_status').stage, 'notStarted');
  assert.equal(invoke('quick_connect_prepare_tools').stage, 'nodeReady');
  assert.equal(invoke('quick_connect_sign_in').stage, 'signedIn');
  assert.throws(() => invoke('quick_connect_pairing_create'), /Set up Quick connect first/);
  const deployed = invoke('quick_connect_start_setup');
  assert.deepEqual([deployed.stage, deployed.setup.status, deployed.connected, deployed.legacyConnect.visible], ['deployed', 'succeeded', true, false]);

  const pairing = invoke('quick_connect_pairing_create', { scope: 'full' });
  assert.match(pairing.link, /^pandacode:\/\/quick-connect\?v=1&relay=https%3A%2F%2F.*&token=PREVIEW-TOKEN-1&host=/);
  assert.ok(pairing.expiresAt - Date.now() <= 300_000);
  assert.ok(invoke('quick_connect_status').pairing);
  const phone = pair();
  // A code works once.
  assert.throws(() => pair(), /not valid or has expired/);
  assert.deepEqual(events.find(([name]) => name === 'quick-connect://paired')[1], { deviceId: phone.deviceId, name: 'Preview phone', scope: 'full' });
  assert.equal(invoke('quick_connect_status').stage, 'paired');
  assert.equal(invoke('quick_connect_devices').length, 1);
  // A pairing ends 30 days after it was made.
  assert.equal(phone.expiresAt - phone.pairedAt, 30 * 86_400_000);
  assert.throws(() => invoke('quick_connect_revoke_device', { deviceId: 'nobody' }), /not paired/);
  assert.equal(invoke('quick_connect_revoke_device', { deviceId: phone.deviceId }).stage, 'deployed');

  assert.equal(invoke('quick_connect_set_enabled', { enabled: false }).connected, false);
  assert.equal(invoke('quick_connect_set_enabled', { enabled: true }).connected, true);
  const removed = invoke('quick_connect_remove', {});
  assert.deepEqual([removed.stage, removed.relay, removed.setup.message], ['signedIn', null, 'Quick connect is removed.']);
  assert.ok(events.filter(([name]) => name === 'quick-connect://status').length >= 8);
});

test('setup stops for an account choice, a missing workers.dev address and failures', () => {
  const several = fixture({ accounts: 2 });
  const asked = several.invoke('quick_connect_start_setup');
  assert.deepEqual([asked.setup.status, asked.canContinue, asked.setup.accounts.length], ['needs-account', true, 2]);
  assert.throws(() => several.invoke('quick_connect_choose_account', { accountId: '../x' }), /not a Cloudflare account/);
  assert.equal(several.invoke('quick_connect_choose_account', { accountId: 'bbbb2222' }).relay.accountId, 'bbbb2222');

  const hooks = { subdomain: true };
  const waiting = fixture(hooks);
  assert.equal(waiting.invoke('quick_connect_start_setup').setup.status, 'needs-subdomain');
  hooks.subdomain = false;
  assert.equal(waiting.invoke('quick_connect_continue').setup.status, 'succeeded');
  assert.throws(() => waiting.invoke('quick_connect_continue'), /nothing to continue/);

  assert.deepEqual(fixture({ fail: 'node' }).invoke('quick_connect_start_setup').setup.action, 'install-node');
  const failed = fixture({ fail: 'deploy' }).invoke('quick_connect_start_setup');
  assert.deepEqual([failed.setup.status, failed.relay, failed.stage], ['failed', null, 'signedIn']);
  assert.equal(fixture({ unreachable: 'network-refused' }).invoke('quick_connect_start_setup').setup.problem, 'network-refused');
});

test('connection choices and the hidden legacy relay are plain settings', () => {
  const { invoke, hooks } = fixture();
  assert.equal(invoke('quick_connect_set_mode', { mode: 'tailscale' }).mode, 'tailscale');
  assert.throws(() => invoke('quick_connect_set_mode', { mode: 'relay' }), /Unknown connection choice/);
  assert.equal(invoke('quick_connect_set_legacy_connect_visible', { visible: true }).legacyConnect.visible, true);
  assert.equal(hooks.calls.length, 3);
});
