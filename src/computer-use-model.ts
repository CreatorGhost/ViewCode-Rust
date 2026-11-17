/* Computer use, the pure part: the shapes `computer_use_*` returns (src-tauri/src/computer_use.rs),
 * the words the settings page and the approval card use, and the small decisions they make.
 * No DOM and no imports, so `tests/computer-use-model.test.mjs` loads it directly. */

export type ComputerUseMode = 'off' | 'observe' | 'control';
export type ComputerUseApprovals = 'thread' | 'risky' | 'never' | 'bypass';
export type ComputerUseScreen = 'allow' | 'ask';
export type ComputerUseDecision = 'accept' | 'acceptForSession' | 'acceptAlways' | 'decline' | 'cancel';

export interface ComputerUseSettings { mode: ComputerUseMode; approvals: ComputerUseApprovals; screen: ComputerUseScreen }
/** `platform` is Rust's `std::env::consts::OS` (`macos`, `linux`, `windows`). */
export interface ComputerUseStatus { mode: ComputerUseMode; platform: string; driverAvailable: boolean; accessibility: 'granted' | 'denied' | 'unknown'; reason?: string }
export interface ComputerUseOption { decision: ComputerUseDecision; label: string; warning?: string }
/** A waiting card. `detail` is the only free text and never carries typed text (the backend counts it). */
export interface ComputerUseApproval { requestId: string; threadId: string; turnId: string; appName: string; detail: string; options: ComputerUseOption[]; createdAt: number }
/** One handled request: command, outcome and numbers only. `outcome` is `ok` or a `CU-*` code. */
export interface ComputerUseActivity { at: number; threadId: string; command: string; outcome: string; durationMs: number; effect?: string; tookFocus?: boolean }
export interface ComputerUseState { settings: ComputerUseSettings; bypass: boolean; status: ComputerUseStatus; approvals: ComputerUseApproval[]; activity: ComputerUseActivity[] }
/** What `computer_use_set_settings` takes; fields left out keep their value. */
export interface ComputerUsePatch { mode?: ComputerUseMode; approvals?: ComputerUseApprovals; screen?: ComputerUseScreen; acknowledgeRisk?: boolean }

export const MODES: { id: ComputerUseMode; label: string }[] = [
  { id: 'off', label: 'Off' },
  { id: 'observe', label: 'Look only' },
  { id: 'control', label: 'Control' },
];
export const MODE_DESCRIPTION: Record<ComputerUseMode, string> = {
  off: 'Agents cannot see or use the apps on this computer.',
  observe: 'Agents can list windows, read their controls and take screenshots. They cannot click or type.',
  control: 'Agents can also click, type, scroll and press keys in other apps. Password managers, System Settings and this app are never controlled.',
};

/** The mode line while bypass is on: nothing is protected any more. */
export const BYPASS_MODE_DESCRIPTION = 'Bypass is on: agents act on any app without asking, password managers, System Settings and this app included.';

/** The approvals choice offered as rows; bypass is its own control, never one of these. */
export const APPROVAL_CHOICES: { id: Exclude<ComputerUseApprovals, 'bypass'>; label: string; detail: string }[] = [
  { id: 'thread', label: 'Follow thread permissions', detail: 'In Full access threads routine actions run without asking; every other mode asks before each action.' },
  { id: 'risky', label: 'Only for risky actions', detail: 'Only actions that look destructive ask, such as delete, send, submit, purchase or sign out.' },
  { id: 'never', label: 'Never', detail: 'Agents act without asking. Protected apps stay protected, and input pauses while any approval waits.' },
];
export const SCREEN_CHOICES: { id: ComputerUseScreen; label: string; detail: string }[] = [
  { id: 'allow', label: 'Whenever needed', detail: 'Agents may bring a window to the front for clicks, shortcuts and typing.' },
  { id: 'ask', label: 'Ask once per task', detail: 'The first action that needs the screen asks; kept in the background, agents act on controls directly.' },
];

/** What bypass removes, in the order the confirmation lists it (docs/computer-use.md, "Bypass all permissions"). */
export const BYPASS_REMOVES: string[] = [
  'Every approval. Nothing asks before input, destructive-looking actions included.',
  'The Full access requirement. Threads in any permission mode act without asking.',
  'The show-on-screen question. Agents may bring any window to the front.',
  'Protection of password managers, Keychain Access, System Settings, login and security dialogs, and this app’s own window. Agents can read and operate them, and can approve the prompts this app shows for other tools, their own included.',
  'The pause while another approval waits, so nothing stops an agent approving itself.',
];
export const BYPASS_KEEPS = 'What stays: a turn must be running, stale or unknown targets are refused, and Plan mode never sends input.';

export const ACCESSIBILITY_GUIDANCE = 'Open System Settings → Privacy & Security → Accessibility and turn on PandaCode, then check again.';
export const SCREEN_RECORDING_GUIDANCE = 'Screenshots need Screen Recording: System Settings → Privacy & Security → Screen Recording. Without it agents act on controls by reference only.';

/** Whether this computer can run it at all (macOS with the helper installed). */
export function available(status: ComputerUseStatus | null | undefined): boolean {
  return Boolean(status?.driverAvailable);
}

/** The status line: unavailable with why, or the Accessibility state. `tone` is a state tone for the leading icon. */
export function describeStatus(status: ComputerUseStatus): { tone: 'ready' | 'near' | 'danger' | ''; title: string; detail: string } {
  if (!status.driverAvailable) {
    const fallback = status.platform === 'macos' ? 'The computer-use helper is not installed with this build.' : 'Computer use runs on macOS only.';
    return { tone: '', title: 'Not available on this computer', detail: status.reason ?? fallback };
  }
  if (status.mode === 'off') return { tone: '', title: 'Ready to turn on', detail: 'Permissions are checked once computer use is on.' };
  switch (status.accessibility) {
    case 'granted': return { tone: 'ready', title: 'Accessibility granted', detail: 'Screenshots also need Screen Recording.' };
    case 'denied': return { tone: 'near', title: 'Accessibility is off for PandaCode', detail: ACCESSIBILITY_GUIDANCE };
    default: return { tone: 'near', title: 'Accessibility not confirmed', detail: status.reason ?? 'Couldn’t confirm Accessibility access. Check again in a moment.' };
  }
}

/** Which card the backend sent: the show-on-screen question, a destructive-looking action, or a routine one. */
export function approvalVariant(approval: Pick<ComputerUseApproval, 'options'>): 'screen' | 'destructive' | 'routine' {
  const decisions = approval.options.map(option => option.decision);
  if (!decisions.includes('accept') && decisions.includes('acceptForSession')) return 'screen';
  if (approval.options.some(option => option.warning) || !decisions.includes('acceptForSession')) return 'destructive';
  return 'routine';
}
export const VARIANT_TITLE: Record<ReturnType<typeof approvalVariant>, string> = {
  routine: 'Allow this computer action?',
  destructive: 'Allow this action? It looks destructive',
  screen: 'Show this task on screen?',
};
/** Decline and keep-in-background are the quiet buttons; everything else is a yes. */
export const isRefusal = (decision: ComputerUseDecision): boolean => decision === 'decline' || decision === 'cancel';

export const approvalsFor = (approvals: readonly ComputerUseApproval[], threadId: string): ComputerUseApproval[] => approvals.filter(approval => approval.threadId === threadId);

/** A tool call of the gate's own tools (`pandacode_computer_click`, `mcp__pandacode__pandacode_computer_type`, …): its command, or ''. */
export function computerCommand(toolName: string): string {
  return /(?:^|_)pandacode_computer_([a-z_]+)$/.exec(toolName)?.[1] ?? '';
}
const LOOKING = new Set(['status', 'list_windows', 'observe', 'screenshot']);
/** "Clicking", "Typing", "Looking at the screen": the indicator's word for a command. */
export function commandWord(command: string): string {
  if (LOOKING.has(command)) return 'Looking at the screen';
  const words: Record<string, string> = { press: 'Pressing a control', set_value: 'Filling a field', type: 'Typing', type_focused: 'Typing', key: 'Pressing keys', scroll: 'Scrolling', scroll_at: 'Scrolling', click: 'Clicking', drag: 'Dragging', move: 'Moving the pointer', focus: 'Bringing a window forward' };
  return words[command] ?? 'Using the computer';
}

/** `ok`, `took focus`, or the refusal code, for a recent-actions row. */
export function outcomeLabel(entry: Pick<ComputerUseActivity, 'outcome' | 'tookFocus'>): string {
  if (entry.outcome !== 'ok') return entry.outcome;
  return entry.tookFocus ? 'ok · took focus' : 'ok';
}
/** "now", "4 min", "2 h", "3 d". */
export function ago(at: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 45) return 'now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `${hours} h` : `${Math.round(hours / 24)} d`;
}
