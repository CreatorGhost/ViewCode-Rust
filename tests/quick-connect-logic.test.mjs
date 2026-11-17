import test from 'node:test';
import assert from 'node:assert/strict';
import {
  anywhereModes, countdownText, describeQuickConnectStatus, describeSlowSetupStep, expiryText, formatMinutes, lastSeenText, minutesSince,
  nextTick, pairingCodeState, resolveView, setupChecklist, shownMode, tabOf,
} from '../src/quick-connect-logic.ts';
import { qrSvg } from '../src/quick-connect-qr.ts';
import { encode } from '../src/vendor/uqr/uqr.js';

const MIN = 60_000, HOUR = 60 * MIN, DAY = 24 * HOUR;
const base = () => ({
  stage: 'notStarted', mode: 'quickConnect', enabled: false, relay: null, attached: false, connected: false, setup: { status: 'idle' }, canContinue: false, tools: null, devices: [], pairing: null,
  modes: { quickConnect: { available: true }, sameWifi: { available: false, reason: 'r' }, tailscale: { available: false, installed: false, reason: 'r' } },
  legacyConnect: { visible: false, signedIn: false },
});
const relay = { name: 'pandacode-quick-connect', url: 'https://pandacode-quick-connect.example.workers.dev', accountId: 'a1' };

test('the countdown shows minutes and seconds left, rounded up, and never goes below 0:00', () => {
  const now = 1_000_000;
  assert.equal(countdownText(now + 300_000, now), '5:00');
  assert.equal(countdownText(now + 299_001, now), '5:00');
  assert.equal(countdownText(now + 299_000, now), '4:59');
  assert.equal(countdownText(now + 61_000, now), '1:01');
  assert.equal(countdownText(now + 9_500, now), '0:10');
  assert.equal(countdownText(now + 1, now), '0:01');
  assert.equal(countdownText(now, now), '0:00');
  assert.equal(countdownText(now - 5_000, now), '0:00');
});

test('the countdown repaints exactly when its text changes', () => {
  assert.equal(nextTick(10_500, 10_000), 500);
  assert.equal(nextTick(12_000, 10_000), 1000);
  assert.equal(nextTick(10_000, 10_000), 0);
});

test('a pairing code is active until it expires, unless a phone used it', () => {
  assert.equal(pairingCodeState(2000, 1000), 'active');
  assert.equal(pairingCodeState(2000, 2000), 'expired');
  assert.equal(pairingCodeState(2000, 1000, true), 'paired');
  assert.equal(pairingCodeState(2000, 5000, true), 'paired');
});

test('expiry text names the day a pairing ends, or the time within a day', () => {
  const now = Date.UTC(2026, 10, 17, 12, 0);
  const options = { locale: 'en-US', timeZone: 'UTC' };
  assert.equal(expiryText(now + 30 * DAY, now, options), 'Pairs again after Dec 17');
  assert.equal(expiryText(now + 4 * DAY, now, options), 'Pairs again after Nov 21');
  assert.equal(expiryText(now + 3 * HOUR, now, options), 'Pairs again after 3:00 PM');
  assert.equal(expiryText(now, now, options), 'Pairing ended; pair again with a new code');
});

test('last seen reads as online, minutes, hours, days or a date', () => {
  const now = Date.UTC(2026, 10, 17, 12, 0);
  const options = { locale: 'en-US', timeZone: 'UTC' };
  assert.equal(lastSeenText({ online: true, lastSeen: now - DAY }, now, options), 'Online now');
  assert.equal(lastSeenText({ online: false, lastSeen: null }, now, options), 'Not seen yet');
  assert.equal(lastSeenText({ online: false, lastSeen: now - 20_000 }, now, options), 'Last seen just now');
  assert.equal(lastSeenText({ online: false, lastSeen: now - 5 * MIN }, now, options), 'Last seen 5 min ago');
  assert.equal(lastSeenText({ online: false, lastSeen: now - 3 * HOUR }, now, options), 'Last seen 3 h ago');
  assert.equal(lastSeenText({ online: false, lastSeen: now - DAY - HOUR }, now, options), 'Last seen yesterday');
  assert.equal(lastSeenText({ online: false, lastSeen: now - 3 * DAY }, now, options), 'Last seen 3 days ago');
  assert.equal(lastSeenText({ online: false, lastSeen: now - 10 * DAY }, now, options), 'Last seen Nov 7');
});

test('the setup checklist follows ViewCode: the tool check counts as signing in', () => {
  assert.deepEqual(setupChecklist('checking-tools', 'Getting ready…'), [
    { label: 'Signing in to Cloudflare', state: 'current', note: 'Getting ready…' },
    { label: 'Setting up your relay', state: 'pending' },
    { label: 'Securing it', state: 'pending' },
    { label: 'Checking it works', state: 'pending' },
  ]);
  assert.deepEqual(setupChecklist('signing-in', 'Signing in to Cloudflare…')[0], { label: 'Signing in to Cloudflare', state: 'current' });
  assert.deepEqual(setupChecklist('storing-secret', 'Securing it…').map(row => row.state), ['done', 'done', 'current', 'pending']);
  assert.equal(setupChecklist('verifying', 'Waiting for your network to allow the new address.')[3].note, 'Waiting for your network to allow the new address.');
  assert.equal(setupChecklist(undefined)[0].state, 'current');
});

test('elapsed minutes and the slow-step note', () => {
  assert.equal(minutesSince(undefined, 5 * MIN), 0);
  assert.equal(minutesSince(0, 3 * MIN + 5), 3);
  assert.equal(minutesSince(10 * MIN, 0), 0);
  assert.equal(formatMinutes(0), null);
  assert.equal(formatMinutes(3), '3 min');
  assert.equal(describeSlowSetupStep('deploying', 1), null);
  assert.match(describeSlowSetupStep('deploying', 2), /Taking longer than usual/);
  assert.equal(describeSlowSetupStep('signing-in', 9), null);
});

test('a status resolves to one screen, setup states first', () => {
  const status = base();
  assert.equal(resolveView(status), 'not-set-up');
  assert.match(describeQuickConnectStatus(status), /^Not set up/);
  assert.equal(resolveView({ ...status, setup: { status: 'running', step: 'deploying', stepStartedAt: 0, message: 'Setting up your relay…' } }), 'running');
  assert.equal(describeQuickConnectStatus({ ...status, setup: { status: 'running', step: 'deploying', stepStartedAt: 0, message: 'Setting up your relay…' } }), 'Setting up your relay…');
  assert.equal(resolveView({ ...status, setup: { status: 'needs-account', accounts: [], message: 'm' } }), 'needs-account');
  assert.equal(resolveView({ ...status, relay }), 'off');
  assert.equal(resolveView({ ...status, relay, enabled: true }), 'connecting');
  assert.equal(resolveView({ ...status, relay, enabled: true, setup: { status: 'unreachable', problem: 'transient', message: 'not answering' } }), 'unreachable');
  const ready = { ...status, relay, enabled: true, attached: true, connected: true };
  assert.equal(resolveView(ready), 'ready');
  assert.equal(describeQuickConnectStatus(ready), 'Connected through pandacode-quick-connect.example.workers.dev.');
});

test('connection choices: Tailscale is offered only when installed', () => {
  const status = base();
  assert.deepEqual(anywhereModes(status), ['quickConnect']);
  assert.deepEqual(anywhereModes({ modes: { ...status.modes, tailscale: { available: false, installed: true } } }), ['quickConnect', 'tailscale']);
  assert.equal(shownMode('tailscale', status), 'quickConnect');
  assert.equal(shownMode('sameWifi', status), 'sameWifi');
  assert.equal(tabOf('sameWifi'), 'local');
  assert.equal(tabOf('tailscale'), 'anywhere');
});

test('the QR SVG encodes the link without containing it', () => {
  const link = 'pandacode://quick-connect?v=1&relay=https%3A%2F%2Fx.workers.dev&token=SECRET-TOKEN-VALUE&host=dev&key=K&name=Mac';
  const svg = qrSvg(link);
  assert.match(svg, /^<svg class="qc-qr-svg" viewBox="0 0 (\d+) \1"/);
  assert.ok(!svg.includes('SECRET-TOKEN-VALUE') && !svg.includes('pandacode://') && !svg.includes('<title'));
  // The drawn modules are exactly the encoder's: count dark cells from the path.
  const { size, data } = encode(link, { ecc: 'M', border: 2 });
  assert.equal(Number(svg.match(/viewBox="0 0 (\d+)/)[1]), size);
  const dark = data.flat().filter(Boolean).length;
  const drawn = [...svg.matchAll(/h(\d+)v1/g)].reduce((sum, match) => sum + Number(match[1]), 0);
  assert.equal(drawn, dark);
});
