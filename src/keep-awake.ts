/**
 * Keep awake (docs/managed-mode.md#keep-awake): while an agent turn runs, and only then,
 * the app holds a macOS idle-sleep assertion so a locked Mac does not cut the turn off.
 */
import { call } from './bridge';
import { escape } from './dom';

export interface KeepAwakeState {
  /** The user's setting (on by default). */
  enabled: boolean;
  /** False when the organization's policy turns keep-awake off. */
  permitted: boolean;
  /** False where this platform has nothing to hold (anything but macOS). */
  supported: boolean;
  /** The computer is being kept awake right now. */
  active: boolean;
  turns: number;
}

const TITLE = 'Keep this computer awake while an agent works';
const ABOUT = 'Held only while a turn runs, and released when the last one ends. It stops idle sleep, such as when the screen locks. Closing the lid on battery still puts the computer to sleep.';

function note(state: KeepAwakeState): string {
  if (!state.supported) return 'Available on macOS.';
  if (!state.permitted) return 'Turned off by your organization.';
  return state.active ? 'Keeping this computer awake for a running turn.' : '';
}

/** Settings → Agents → Advanced → Power: the one switch. */
export function mountKeepAwake(host: HTMLElement): () => void {
  let disposed = false, saving = false, loaded = false;
  host.innerHTML = `<div class="settings-group"><div class="setting-row"><div><strong>${TITLE}</strong><small id="keep-awake-about">${escape(ABOUT)}</small></div><input type="checkbox" class="settings-switch" id="keep-awake-enabled" aria-label="${TITLE}" aria-describedby="keep-awake-about keep-awake-status" disabled></div></div><p class="harness-privacy-status" id="keep-awake-status" role="status"></p>`;
  const toggle = host.querySelector<HTMLInputElement>('#keep-awake-enabled')!;
  const status = host.querySelector<HTMLElement>('#keep-awake-status')!;
  const render = (state: KeepAwakeState) => {
    loaded = true;
    toggle.checked = state.enabled && state.permitted;
    toggle.disabled = !state.permitted || !state.supported;
    status.textContent = note(state);
  };
  call<KeepAwakeState>('keep_awake_state').then(state => { if (!disposed) render(state); })
    .catch(() => { if (!disposed) status.textContent = 'This setting couldn’t load.'; });
  toggle.addEventListener('change', () => {
    if (saving || !loaded) { toggle.checked = !toggle.checked; return; }
    const wanted = toggle.checked;
    saving = true;
    call<KeepAwakeState>('keep_awake_set_enabled', { enabled: wanted })
      .then(state => { if (!disposed) render(state); })
      .catch(() => { if (!disposed) { toggle.checked = !wanted; status.textContent = 'Couldn’t save this setting.'; } })
      .finally(() => { saving = false; });
  });
  return () => { disposed = true; };
}
