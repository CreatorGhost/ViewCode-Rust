/**
 * The launch gate (docs/managed-mode.md): an agent CLI is started only when the user
 * turned it on and no organization policy forbids it. This module is the IPC surface; the
 * decision itself is native (`src-tauri/src/launch_gate.rs`).
 */
import { call, subscribe } from './bridge';

export interface LaunchGateHarness {
  id: string;
  name: string;
  /** Found on disk. The binary is never run to answer this. */
  installed: boolean;
  path: string | null;
  /** May be started now: turned on and permitted. */
  enabled: boolean;
  /** Whether the user may change `enabled`; false when the organization decides. */
  changeable: boolean;
  /** Why it cannot be turned on. */
  locked?: 'blocked' | 'notAllowed' | 'policyUnreadable';
  /** This profile used the harness before the gate existed (a first-run preselection). */
  suggested: boolean;
}

export interface LaunchGateState {
  enabled: string[];
  /** The first-run choice has not been made: the welcome step still has to ask. */
  firstRunPending: boolean;
  /** This computer looks managed by an organization, or a policy is installed. */
  managed: boolean;
  managedEvidence: string[];
  /** Nothing is turned on for the user, not even what the profile used before. */
  strict: boolean;
  policySource: string | null;
  policyProblem: string | null;
  /** The policy's allow list, when it pins one. */
  pinned: string[] | null;
  blocked: string[];
  harnesses: LaunchGateHarness[];
}

export const LAUNCH_GATE_CHANGED = 'launch-gate-changed';

export const launchGateState = () => call<LaunchGateState>('launch_gate_state');
export const setHarnessEnabled = (harness: string, enabled: boolean) => call<LaunchGateState>('launch_gate_set_enabled', { harness, enabled });
/** The first-run choice: exactly these harnesses on. Refused once a choice exists. */
export const completeFirstRun = (enabled: string[]) => call<LaunchGateState>('launch_gate_complete_first_run', { enabled });
export const onLaunchGateChanged = (callback: (state: LaunchGateState) => void) => subscribe(LAUNCH_GATE_CHANGED, callback);

/** The one line a harness card shows when the harness may not be started. */
export function gateLabel(harness: LaunchGateHarness | undefined): { text: string; tone: string } | null {
  if (!harness || harness.enabled) return null;
  if (harness.locked === 'blocked') return { text: 'Blocked by your organization', tone: 'warn' };
  if (harness.locked === 'notAllowed') return { text: 'Not allowed by your organization', tone: 'warn' };
  if (harness.locked === 'policyUnreadable') return { text: 'Policy unreadable', tone: 'danger' };
  return { text: 'Not enabled', tone: '' };
}
