// Preview data for Quick connect (src-tauri/src/quick_connect/): every `quick_connect_*` command, with no
// Cloudflare, Node download or relay behind it. By default setup runs to its end state at once; tests steer it with
// `window.__quickConnect = { accounts?: 2, subdomain?: true, fail?: 'deploy' | 'node', unreachable?: QuickConnectProblem,
//   steps?: true, signInDenied?: true, removeFails?: true, pairingTtlMs?: number, tailscaleInstalled?: true,
//   legacyVisible?: true, preset?: 'ready' | 'off' }`
// and read `window.__quickConnectCalls`. With `steps`, setup stops on each running step (as the real one reports
// them) until `__quickConnectAdvance()`; `signInDenied` ends the sign-in step as Cloudflare refusing it. `preset`
// starts with a relay already set up (and, for `ready`, two paired phones). `emit` sends `quick-connect://status`
// after every change.
import type { QuickConnectDevice, QuickConnectPairing, QuickConnectSetup, QuickConnectStatus, QuickConnectStep } from './quick-connect-types';

const URL = 'https://pandacode-quick-connect.preview.workers.dev';
const NEEDS_LISTENER = 'Not available yet: this needs the app\'s local network listener, which is not built.';
const DAY = 86_400_000;
/** The step messages setup.rs reports (ViewCode's wording). */
const STEP_MESSAGE: Record<QuickConnectStep, string> = {
  'checking-tools': 'Getting ready…', 'signing-in': 'Signing in to Cloudflare…', deploying: 'Setting up your relay…',
  'storing-secret': 'Securing it…', verifying: 'Checking it works…', removing: 'Removing Quick connect…',
};
// wrangler's device flow, as wrangler.rs reads it: a fixed sample code, never a real one.
const SIGN_IN = { url: 'https://dash.cloudflare.com/oauth2/device', code: 'WDJB-MJHT', openUrl: 'https://dash.cloudflare.com/oauth2/device?user_code=WDJB-MJHT' };
const fresh = (hooks: Record<string, any>): QuickConnectStatus => {
  const state: QuickConnectStatus = {
    stage: 'notStarted', mode: 'quickConnect', enabled: false, relay: null, attached: false, connected: false, setup: { status: 'idle' }, canContinue: false, tools: null, devices: [], pairing: null,
    modes: { quickConnect: { available: true }, sameWifi: { available: false, reason: NEEDS_LISTENER }, tailscale: { available: false, installed: !!hooks.tailscaleInstalled, reason: NEEDS_LISTENER } },
    legacyConnect: { visible: !!hooks.legacyVisible, signedIn: false },
  };
  if (hooks.preset === 'ready' || hooks.preset === 'off') {
    const now = Date.now(), on = hooks.preset === 'ready';
    Object.assign(state, { stage: on ? 'paired' : 'deployed', enabled: on, attached: on, connected: on, relay: { name: 'pandacode-quick-connect', url: URL, accountId: 'aaaa1111' }, tools: { source: 'system', nodeVersion: '22.15.0' } });
    if (on) state.devices = [
      { deviceId: 'phone-a', name: 'Preview iPhone', platform: 'ios', fingerprint: '5e1f0c9a2b7d4e63', scope: 'full', pairedAt: now - 2 * DAY, online: true, lastSeen: now, expiresAt: now + 28 * DAY },
      { deviceId: 'phone-b', name: 'Preview Pixel', platform: 'android', fingerprint: '0b9e7c41d2a35f86', scope: 'full', pairedAt: now - 26 * DAY, online: false, lastSeen: now - 3 * DAY, expiresAt: now + 4 * DAY },
    ];
  }
  return state;
};

export function createPreviewQuickConnect(emit: (name: string, payload: unknown) => void, hooks: () => Record<string, any> = () => ({})) {
  let state = fresh(hooks());
  let pending: { accountId?: string } | undefined;
  let pairings = 0;
  /** A stepped operation waiting for `advance()`: the steps still to show, then its end. */
  let run: { steps: QuickConnectStep[]; index: number; finish: () => QuickConnectStatus } | undefined;
  const stage = () => { state.stage = state.relay ? (state.devices.length ? 'paired' : 'deployed') : state.stage === 'notStarted' || state.stage === 'nodeReady' ? state.stage : 'signedIn'; };
  const done = (setup?: QuickConnectSetup) => {
    if (setup) state.setup = setup;
    stage();
    state.canContinue = pending !== undefined;
    emit('quick-connect://status', structuredClone(state));
    return structuredClone(state);
  };
  const show = () => {
    const step = run!.steps[run!.index];
    return done({ status: 'running', step, stepStartedAt: Date.now(), message: STEP_MESSAGE[step], ...(step === 'signing-in' ? { signIn: { ...SIGN_IN } } : {}) });
  };
  /** Runs `finish` now, or with `steps` shows each step first and runs it after the last `advance()`. */
  const stepped = (steps: QuickConnectStep[], finish: () => QuickConnectStatus) => {
    if (!hooks().steps || !steps.length) return finish();
    run = { steps, index: 0, finish };
    return show();
  };
  const advance = () => {
    if (!run) throw new Error('No Quick connect step is running.');
    const left = run.steps[run.index];
    if (left === 'signing-in' && hooks().signInDenied) { run = undefined; pending = undefined; return done({ status: 'failed', message: 'Cloudflare sign-in did not finish. Try again.', details: 'Login denied.' }); }
    if (++run.index < run.steps.length) return show();
    const { finish } = run; run = undefined;
    return finish();
  };
  const connect = () => {
    const test = hooks();
    state.enabled = true;
    if (test.unreachable) { state.attached = state.connected = false; return done({ status: 'unreachable', problem: test.unreachable, message: test.unreachable === 'credential-rejected' ? 'The relay does not accept this computer\'s secret. Redeploy with a new secret to fix it.' : 'Your relay is set up, but its address is not answering yet. Try again later from Settings.' }); }
    state.attached = state.connected = true;
    return done({ status: 'succeeded', message: 'Quick connect is set up.' });
  };
  const deploy = (accountId?: string) => {
    const test = hooks();
    pending = undefined;
    if (test.fail === 'node') return done({ status: 'failed', message: 'Quick connect setup needs Node.js 22 or newer (nodejs.org). Install it, then try again.', action: 'install-node' });
    state.tools = { source: 'system', nodeVersion: '22.15.0' };
    if (state.stage === 'notStarted' || state.stage === 'nodeReady') state.stage = 'signedIn';
    if (test.accounts === 2 && !accountId && !state.relay?.accountId) {
      pending = {};
      return done({ status: 'needs-account', accounts: [{ id: 'aaaa1111', name: 'Personal' }, { id: 'bbbb2222', name: 'Work' }], message: 'Your Cloudflare sign-in has several accounts. Choose one for the relay.' });
    }
    if (test.subdomain) {
      pending = { accountId };
      return done({ status: 'needs-subdomain', message: 'Your Cloudflare account needs a free workers.dev address first. Open Cloudflare, choose one under Workers & Pages, then come back.' });
    }
    if (test.fail === 'deploy') return done({ status: 'failed', message: 'Cloudflare did not accept the relay. Try again.', details: 'Authentication error [code: 10000]' });
    return stepped(['deploying', 'storing-secret', 'verifying'], () => {
      state.relay = { name: 'pandacode-quick-connect', url: URL, accountId: accountId ?? state.relay?.accountId ?? 'aaaa1111' };
      return connect();
    });
  };
  /** Test seam: what the phone does after scanning the link. */
  const pair = (name = 'Preview phone'): QuickConnectDevice => {
    if (!state.pairing || state.pairing.expiresAt <= Date.now()) throw new Error('That pairing code is not valid or has expired.');
    const device: QuickConnectDevice = { deviceId: `phone-${state.devices.length + 1}`, name, platform: 'ios', fingerprint: '5e1f0c9a2b7d4e63', scope: 'full', pairedAt: Date.now(), online: true, lastSeen: Date.now(), expiresAt: Date.now() + 30 * DAY };
    state.devices.push(device);
    state.pairing = null;
    emit('quick-connect://paired', { deviceId: device.deviceId, name: device.name, scope: device.scope });
    done();
    return device;
  };
  function invoke(command: string, args: Record<string, any> = {}): unknown {
    const test = hooks();
    (test.calls ??= []).push({ command, ...args });
    if (run && /^quick_connect_(start_setup|continue|choose_account|set_enabled|remove|prepare_tools|sign_in)$/.test(command)) throw new Error('Quick connect setup is already running.');
    switch (command) {
      case 'quick_connect_status': return structuredClone(state);
      case 'quick_connect_prepare_tools':
        if (test.fail === 'node') return deploy();
        state.tools = { source: 'system', nodeVersion: '22.15.0' };
        if (state.stage === 'notStarted') state.stage = 'nodeReady';
        return done({ status: 'idle', message: 'Cloudflare\'s tools are ready.' });
      case 'quick_connect_sign_in':
        state.tools = { source: 'system', nodeVersion: '22.15.0' };
        if (!state.relay) state.stage = 'signedIn';
        return done({ status: 'idle', message: 'Signed in to Cloudflare.' });
      case 'quick_connect_start_setup': return stepped(['checking-tools', 'signing-in'], () => deploy());
      case 'quick_connect_continue':
        if (!pending) throw new Error('There is nothing to continue.');
        return deploy(pending.accountId);
      case 'quick_connect_choose_account':
        if (!pending) throw new Error('There is nothing to continue.');
        if (!/^[a-z0-9]{1,64}$/i.test(String(args.accountId ?? ''))) throw new Error('That is not a Cloudflare account.');
        return deploy(args.accountId);
      case 'quick_connect_cancel': run = undefined; pending = undefined; return done({ status: 'idle' });
      case 'quick_connect_set_enabled':
        if (!state.relay) throw new Error(args.enabled ? 'No relay is set up on this computer yet. Set up Quick connect instead.' : 'Set up Quick connect first.');
        if (args.enabled) return stepped(['verifying'], connect);
        state.enabled = false; state.attached = state.connected = false; state.pairing = null;
        return done({ status: 'idle' });
      case 'quick_connect_remove':
        if (test.removeFails && !args.localOnly) return done({ status: 'remove-failed', message: 'Could not delete the relay from Cloudflare. Nothing changed on this computer. Try again, or remove it from this computer only.', details: 'Network error' });
        state = { ...state, relay: null, enabled: false, attached: false, connected: false, devices: [], pairing: null, stage: 'signedIn' };
        return done({ status: 'idle', message: args.localOnly ? 'Quick connect is removed from this computer. The relay is still on your Cloudflare account.' : 'Quick connect is removed.' });
      case 'quick_connect_pairing_create': {
        if (!state.relay || !state.enabled) throw new Error('Set up Quick connect first.');
        const scope = args.scope ?? 'full';
        if (!['view', 'control', 'full'].includes(scope)) throw new Error('A pairing can give view, control or full access.');
        const expiresAt = Date.now() + (test.pairingTtlMs ?? 300_000);
        state.pairing = { expiresAt };
        done();
        // A recognisably fake token: previews and screenshots must never look like a usable link.
        const pairing: QuickConnectPairing = { link: `pandacode://quick-connect?v=1&relay=${encodeURIComponent(URL)}&token=PREVIEW-TOKEN-${++pairings}&host=dev-this&key=PREVIEW&name=This%20Mac`, expiresAt, scope };
        return pairing;
      }
      case 'quick_connect_pairing_cancel': state.pairing = null; return done();
      case 'quick_connect_devices': return structuredClone(state.devices);
      case 'quick_connect_revoke_device': {
        if (!state.devices.some(device => device.deviceId === args.deviceId)) throw new Error('That phone is not paired with this computer.');
        state.devices = state.devices.filter(device => device.deviceId !== args.deviceId);
        return done();
      }
      case 'quick_connect_set_mode':
        if (!['quickConnect', 'sameWifi', 'tailscale'].includes(args.mode)) throw new Error('Unknown connection choice.');
        state.mode = args.mode;
        return done();
      case 'quick_connect_set_legacy_connect_visible': state.legacyConnect.visible = !!args.visible; return done();
      default: throw new Error(`Unknown Quick connect command: ${command}`);
    }
  }
  return { invoke, pair, advance };
}
