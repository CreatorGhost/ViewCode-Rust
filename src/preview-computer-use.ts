/* Browser-preview mocks for computer use: `computer_use_state`, `computer_use_set_settings`,
 * `computer_use_approvals`, `computer_use_respond` and the `computer-use-changed` event, shaped like
 * src-tauri/src/computer_use.rs (wording and options as policy.rs sends them). Nothing touches the screen.
 *
 * `?cu=` picks a scenario: `unavailable` (Linux, no helper), `missing` (macOS without the helper), `off` (the
 * default), `observe`, `denied` (look only, Accessibility off), `control` (a routine approval waits in
 * session-1), `destructive`, `screen` (the show-on-screen question) or `bypass` (bypass on, nothing waits).
 * Hooks: `__cuCalls` records every call; `__cuApprove(variant, threadId?)` opens a card and emits;
 * `__cuResolve(requestId?)` closes cards as if answered elsewhere, timed out or ended, and emits;
 * `__cuTool(threadId, command, status)` plays a tool event of the gate's tools. */

type Emit = (name: string, event: unknown) => void;
type Mode = 'off' | 'observe' | 'control';
type Approvals = 'thread' | 'risky' | 'never' | 'bypass';
type Option = { decision: string; label: string; warning?: string };
type Approval = { requestId: string; threadId: string; turnId: string; appName: string; detail: string; options: Option[]; createdAt: number };

const ROUTINE: Option[] = [{ decision: 'accept', label: 'Allow once' }, { decision: 'acceptForSession', label: 'Allow for the rest of this turn' }, { decision: 'decline', label: 'Decline' }];
const DESTRUCTIVE: Option[] = [{ decision: 'accept', label: 'Allow once', warning: 'Looks destructive, so this always asks' }, { decision: 'decline', label: 'Decline' }];
const SCREEN: Option[] = [{ decision: 'acceptForSession', label: 'Show on screen for this task' }, { decision: 'decline', label: 'Keep it in the background' }];
const DETAIL: Record<string, string> = {
  routine: 'Type 13 characters into text field "Search" in Safari — "Start Page"',
  destructive: 'Press button "Delete" in Notes — "Shopping"',
  screen: 'Show this on your screen? To work in Safari, the agent needs to bring its windows to the front and use the mouse and keyboard until this response ends. In the background it can only act on controls directly.',
};
const OPTIONS: Record<string, Option[]> = { routine: ROUTINE, destructive: DESTRUCTIVE, screen: SCREEN };

export function createPreviewComputerUse(emit: Emit, scenario: string | null) {
  const w = window as any;
  const unavailable = scenario === 'unavailable', missing = scenario === 'missing';
  const settings: { mode: Mode; approvals: Approvals; screen: 'allow' | 'ask' } = {
    mode: scenario === 'observe' || scenario === 'denied' ? 'observe' : ['control', 'destructive', 'screen', 'bypass'].includes(scenario ?? '') ? 'control' : 'off',
    approvals: scenario === 'bypass' ? 'bypass' : 'thread',
    screen: scenario === 'screen' ? 'ask' : 'allow',
  };
  let approvals: Approval[] = [];
  let next = 1;
  const now = Date.now();
  const activity: Record<string, unknown>[] = settings.mode === 'off' ? [] : [
    { at: now - 20_000, threadId: 'session-1', command: 'screenshot', outcome: 'ok', durationMs: 412 },
    { at: now - 45_000, threadId: 'session-1', command: 'press', outcome: 'ok', effect: 'dispatched', durationMs: 640, tookFocus: false },
    { at: now - 70_000, threadId: 'session-1', command: 'click', outcome: 'CU-CON-008', effect: 'not-dispatched', durationMs: 12 },
    { at: now - 600_000, threadId: 'session-2', command: 'list-windows', outcome: 'ok', durationMs: 120 },
  ];
  const changed = (sessionId: string | null) => emit('computer-use-changed', { sessionId });
  const open = (variant: string, threadId = 'session-1') => {
    const approval = { requestId: `cu-${next++}`, threadId, turnId: `turn-${threadId}`, appName: 'Computer use', detail: DETAIL[variant] ?? DETAIL.routine, options: OPTIONS[variant] ?? ROUTINE, createdAt: Date.now() };
    approvals.push(approval);
    changed(threadId);
    return approval.requestId;
  };
  if (scenario === 'control') approvals.push({ requestId: `cu-${next++}`, threadId: 'session-1', turnId: 'turn-session-1', appName: 'Computer use', detail: DETAIL.routine, options: ROUTINE, createdAt: now });
  if (scenario === 'destructive') approvals.push({ requestId: `cu-${next++}`, threadId: 'session-1', turnId: 'turn-session-1', appName: 'Computer use', detail: DETAIL.destructive, options: DESTRUCTIVE, createdAt: now });
  if (scenario === 'screen') approvals.push({ requestId: `cu-${next++}`, threadId: 'session-1', turnId: 'turn-session-1', appName: 'Computer use', detail: DETAIL.screen, options: SCREEN, createdAt: now });

  const status = () => {
    if (unavailable) return { mode: settings.mode, platform: 'linux', driverAvailable: false, accessibility: 'unknown', reason: 'Computer use is not supported on Linux yet.' };
    if (missing) return { mode: settings.mode, platform: 'macos', driverAvailable: false, accessibility: 'unknown', reason: 'The computer-use helper is not installed with this build.' };
    // As the backend: while off, nothing is asked of the helper.
    if (settings.mode === 'off') return { mode: 'off', platform: 'macos', driverAvailable: true, accessibility: 'unknown' };
    return scenario === 'denied' ? { mode: settings.mode, platform: 'macos', driverAvailable: true, accessibility: 'denied', reason: 'Grant Accessibility.' } : { mode: settings.mode, platform: 'macos', driverAvailable: true, accessibility: 'granted' };
  };
  const copy = <T>(value: T): T => structuredClone(value);
  w.__cuCalls ??= [];
  w.__cuApprove = open;
  w.__cuResolve = (requestId?: string) => {
    const gone = approvals.filter(item => !requestId || item.requestId === requestId);
    approvals = approvals.filter(item => !gone.includes(item));
    for (const thread of new Set(gone.map(item => item.threadId))) changed(thread);
  };
  w.__cuTool = (threadId: string, command: string, status = 'running', id = `tool-${command}`) => emit('agent-event', { sessionId: threadId, kind: 'tool', text: '', tool: { id, name: `mcp__pandacode__pandacode_computer_${command}`, status, input: '{}', output: '' } });

  return async function invoke(command: string, args: Record<string, any>): Promise<unknown> {
    w.__cuCalls.push({ command, args: copy(args) });
    switch (command) {
      case 'computer_use_state': return copy({ settings, bypass: settings.approvals === 'bypass', status: status(), approvals, activity });
      case 'computer_use_approvals': return copy(approvals);
      case 'computer_use_set_settings': {
        const patch = args.settings ?? {};
        const allowed = new Set(['mode', 'approvals', 'screen', 'acknowledgeRisk']);
        if (Object.keys(patch).some(key => !allowed.has(key))) throw new Error('unknown field');
        if (patch.approvals === 'bypass' && patch.acknowledgeRisk !== true) throw new Error('Bypass all permissions removes every computer-use safeguard: nothing asks, and agents can operate password managers, system security dialogs and this app\'s own window. To turn it on, send acknowledgeRisk: true with the change.');
        if (patch.mode) settings.mode = patch.mode;
        if (patch.approvals) settings.approvals = patch.approvals;
        if (patch.screen) settings.screen = patch.screen;
        // Turning it off closes every approval.
        if (settings.mode === 'off') approvals = [];
        changed(null);
        return copy(settings);
      }
      case 'computer_use_respond': {
        const approval = approvals.find(item => item.requestId === args.requestId && item.threadId === args.sessionId);
        if (!approval) throw new Error('This approval is no longer pending.');
        approvals = approvals.filter(item => item !== approval);
        activity.unshift({ at: Date.now(), threadId: approval.threadId, command: approval.options === SCREEN ? 'click' : 'type', outcome: args.decision === 'decline' ? 'CU-CON-002' : 'ok', durationMs: 300 });
        changed(approval.threadId);
        return null;
      }
    }
    throw new Error(`Unknown computer-use command ${command}`);
  };
}
