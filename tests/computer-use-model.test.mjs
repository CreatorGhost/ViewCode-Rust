import test from 'node:test';
import assert from 'node:assert/strict';
import { approvalVariant, approvalsFor, available, ago, BYPASS_REMOVES, commandWord, computerCommand, describeStatus, isRefusal, outcomeLabel } from '../src/computer-use-model.ts';

// The option lists exactly as policy.rs sends them.
const ROUTINE = [{ decision: 'accept', label: 'Allow once' }, { decision: 'acceptForSession', label: 'Allow for the rest of this turn' }, { decision: 'decline', label: 'Decline' }];
const DESTRUCTIVE = [{ decision: 'accept', label: 'Allow once', warning: 'Looks destructive, so this always asks' }, { decision: 'decline', label: 'Decline' }];
const SCREEN = [{ decision: 'acceptForSession', label: 'Show on screen for this task' }, { decision: 'decline', label: 'Keep it in the background' }];

test('each card variant is told apart by the options the backend sent', () => {
  assert.equal(approvalVariant({ options: ROUTINE }), 'routine');
  assert.equal(approvalVariant({ options: DESTRUCTIVE }), 'destructive');
  assert.equal(approvalVariant({ options: SCREEN }), 'screen');
  // A destructive list without its warning is still never "the rest of this turn".
  assert.equal(approvalVariant({ options: DESTRUCTIVE.map(({ decision, label }) => ({ decision, label })) }), 'destructive');
  assert.deepEqual(['accept', 'acceptForSession', 'acceptAlways', 'decline', 'cancel'].map(isRefusal), [false, false, false, true, true]);
});

test('cards belong to the thread whose agent asked', () => {
  const approvals = [{ requestId: 'a', threadId: 't1' }, { requestId: 'b', threadId: 't2' }, { requestId: 'c', threadId: 't1' }];
  assert.deepEqual(approvalsFor(approvals, 't1').map(item => item.requestId), ['a', 'c']);
  assert.deepEqual(approvalsFor(approvals, 't3'), []);
});

test('status reads as unavailable, ready to turn on, or the Accessibility state', () => {
  const linux = { mode: 'off', platform: 'linux', driverAvailable: false, accessibility: 'unknown', reason: 'Computer use is not supported on Linux yet.' };
  assert.equal(available(linux), false);
  assert.deepEqual(describeStatus(linux), { tone: '', title: 'Not available on this computer', detail: 'Computer use is not supported on Linux yet.' });
  assert.match(describeStatus({ ...linux, reason: undefined, platform: 'macos' }).detail, /helper is not installed/);
  assert.match(describeStatus({ ...linux, reason: undefined, platform: 'windows' }).detail, /macOS only/);
  const mac = { mode: 'control', platform: 'macos', driverAvailable: true, accessibility: 'granted' };
  assert.equal(available(mac), true);
  assert.equal(describeStatus({ ...mac, mode: 'off', accessibility: 'unknown' }).title, 'Ready to turn on');
  assert.equal(describeStatus(mac).tone, 'ready');
  const denied = describeStatus({ ...mac, accessibility: 'denied' });
  assert.equal(denied.tone, 'near');
  assert.match(denied.detail, /Privacy & Security → Accessibility/);
  assert.equal(describeStatus({ ...mac, accessibility: 'unknown', reason: 'Helper restarting.' }).detail, 'Helper restarting.');
  assert.equal(available(null), false);
});

test('the gate tools are recognised whatever prefix a harness gives them', () => {
  assert.equal(computerCommand('pandacode_computer_click'), 'click');
  assert.equal(computerCommand('mcp__pandacode__pandacode_computer_type_focused'), 'type_focused');
  assert.equal(computerCommand('computer_click'), '');
  assert.equal(computerCommand('Bash'), '');
  assert.equal(commandWord('screenshot'), 'Looking at the screen');
  assert.equal(commandWord('type'), 'Typing');
  assert.equal(commandWord('something_new'), 'Using the computer');
});

test('recent actions read as outcome and age, never more', () => {
  assert.equal(outcomeLabel({ outcome: 'ok' }), 'ok');
  assert.equal(outcomeLabel({ outcome: 'ok', tookFocus: true }), 'ok · took focus');
  assert.equal(outcomeLabel({ outcome: 'CU-CON-008', tookFocus: true }), 'CU-CON-008');
  const now = 10_000_000;
  assert.deepEqual([0, 30_000, 5 * 60_000, 3 * 3600_000, 50 * 3600_000].map(delta => ago(now - delta, now)), ['now', 'now', '5 min', '3 h', '2 d']);
});

test('the bypass confirmation names everything it removes', () => {
  const text = BYPASS_REMOVES.join(' ');
  for (const part of ['Every approval', 'destructive', 'Full access', 'show-on-screen', 'password managers', 'security dialogs', 'own window', 'pause while another approval waits']) assert.ok(text.includes(part), part);
});
