// Preview data for Quick connect (src-tauri/src/quick_connect/): every `quick_connect_*` command, with no
// Cloudflare, Node download or relay behind it. Setup runs to its end state at once; tests steer it with
// `window.__quickConnect = { accounts?: 2, subdomain?: true, fail?: 'deploy' | 'node', unreachable?: QuickConnectProblem }`
// and read `window.__quickConnectCalls`. `emit` sends `quick-connect://status` after every change.
import type { QuickConnectDevice, QuickConnectPairing, QuickConnectSetup, QuickConnectStatus } from './quick-connect-types';

const URL = 'https://pandacode-quick-connect.preview.workers.dev';
const NEEDS_LISTENER = 'Not available yet: this needs the app\'s local network listener, which is not built.';
const fresh = (): QuickConnectStatus => ({
  stage: 'notStarted', mode: 'quickConnect', enabled: false, relay: null, attached: false, connected: false, setup: { status: 'idle' }, canContinue: false, tools: null, devices: [], pairing: null,
  modes: { quickConnect: { available: true }, sameWifi: { available: false, reason: NEEDS_LISTENER }, tailscale: { available: false, installed: false, reason: NEEDS_LISTENER } },
  legacyConnect: { visible: false, signedIn: false },
});

export function createPreviewQuickConnect(emit: (name: string, payload: unknown) => void, hooks: () => Record<string, any> = () => ({})) {
  let state = fresh();
  let pending: { accountId?: string } | undefined;
  let pairings = 0;
  const stage = () => { state.stage = state.relay ? (state.devices.length ? 'paired' : 'deployed') : state.stage === 'notStarted' || state.stage === 'nodeReady' ? state.stage : 'signedIn'; };
  const done = (setup?: QuickConnectSetup) => {
    if (setup) state.setup = setup;
    stage();
    state.canContinue = pending !== undefined;
    emit('quick-connect://status', structuredClone(state));
    return structuredClone(state);
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
    state.relay = { name: 'pandacode-quick-connect', url: URL, accountId: accountId ?? state.relay?.accountId ?? 'aaaa1111' };
    state.enabled = true;
    if (test.unreachable) return done({ status: 'unreachable', problem: test.unreachable, message: 'Your relay is set up, but its address is not answering yet. Try again later from Settings.' });
    state.attached = state.connected = true;
    return done({ status: 'succeeded', message: 'Quick connect is set up.' });
  };
  /** Test seam: what the phone does after scanning the link. */
  const pair = (name = 'Preview phone'): QuickConnectDevice => {
    if (!state.pairing) throw new Error('That pairing code is not valid or has expired.');
    const device: QuickConnectDevice = { deviceId: `phone-${state.devices.length + 1}`, name, platform: 'ios', fingerprint: '5e1f0c9a2b7d4e63', scope: 'full', pairedAt: Date.now(), online: true, lastSeen: Date.now(), expiresAt: Date.now() + 30 * 86_400_000 };
    state.devices.push(device);
    state.pairing = null;
    emit('quick-connect://paired', { deviceId: device.deviceId, name: device.name, scope: device.scope });
    done();
    return device;
  };
  function invoke(command: string, args: Record<string, any> = {}): unknown {
    const test = hooks();
    (test.calls ??= []).push({ command, ...args });
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
      case 'quick_connect_start_setup': return deploy();
      case 'quick_connect_continue':
        if (!pending) throw new Error('There is nothing to continue.');
        return deploy(pending.accountId);
      case 'quick_connect_choose_account':
        if (!pending) throw new Error('There is nothing to continue.');
        if (!/^[a-z0-9]{1,64}$/i.test(String(args.accountId ?? ''))) throw new Error('That is not a Cloudflare account.');
        return deploy(args.accountId);
      case 'quick_connect_cancel': pending = undefined; return done({ status: 'idle' });
      case 'quick_connect_set_enabled':
        if (!state.relay) throw new Error(args.enabled ? 'No relay is set up on this computer yet. Set up Quick connect instead.' : 'Set up Quick connect first.');
        state.enabled = !!args.enabled; state.attached = state.connected = state.enabled;
        if (!state.enabled) state.pairing = null;
        return done(state.enabled ? { status: 'succeeded', message: 'Quick connect is set up.' } : undefined);
      case 'quick_connect_remove':
        state = { ...state, relay: null, enabled: false, attached: false, connected: false, devices: [], pairing: null, stage: 'signedIn' };
        return done({ status: 'idle', message: args.localOnly ? 'Quick connect is removed from this computer. The relay is still on your Cloudflare account.' : 'Quick connect is removed.' });
      case 'quick_connect_pairing_create': {
        if (!state.relay || !state.enabled) throw new Error('Set up Quick connect first.');
        const scope = args.scope ?? 'full';
        if (!['view', 'control', 'full'].includes(scope)) throw new Error('A pairing can give view, control or full access.');
        const expiresAt = Date.now() + 300_000;
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
  return { invoke, pair };
}
