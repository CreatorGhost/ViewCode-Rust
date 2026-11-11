/* Shapes of the Quick connect commands and events (src-tauri/src/quick_connect/, docs/remote-access.md#quick-connect).
   No UI uses them yet: this file and src/preview-quick-connect.ts are the contract the Connect phone UI builds on. */
export type QuickConnectStage = 'notStarted' | 'nodeReady' | 'signedIn' | 'deployed' | 'paired';
export type QuickConnectMode = 'quickConnect' | 'sameWifi' | 'tailscale';
export type QuickConnectStep = 'checking-tools' | 'signing-in' | 'deploying' | 'storing-secret' | 'verifying' | 'removing';
export type QuickConnectProblem = 'network-refused' | 'credential-rejected' | 'transient';
export type QuickConnectScope = 'view' | 'control' | 'full';
/** The Cloudflare page to open and, with the device flow, the code to check there. */
export interface QuickConnectSignIn { url?: string; code?: string; openUrl?: string }
export interface QuickConnectAccount { id: string; name: string }
export type QuickConnectSetup =
  | { status: 'idle'; message?: string }
  | { status: 'running'; step: QuickConnectStep; stepStartedAt: number; message: string; signIn?: QuickConnectSignIn; problem?: QuickConnectProblem }
  | { status: 'needs-account'; accounts: QuickConnectAccount[]; message: string }
  | { status: 'needs-subdomain'; message: string }
  | { status: 'succeeded'; message: string }
  | { status: 'unreachable'; problem: QuickConnectProblem; message: string; details?: string }
  | { status: 'failed'; message: string; details?: string; action?: 'install-node' }
  | { status: 'remove-failed'; message: string; details?: string };
/** A paired phone. `lastSeen` is null until the relay has seen it. `expiresAt` is 30 days after pairing: nothing extends it
 *  (not use, not rotating the host secret); after it the phone is no longer listed and pairs again with a new code. Times in ms. */
export interface QuickConnectDevice { deviceId: string; name: string; platform: string; fingerprint: string; scope: QuickConnectScope; pairedAt: number; online: boolean; lastSeen: number | null; expiresAt: number }
/** `quick_connect_status`, the result of every other `quick_connect_*` command, and the `quick-connect://status` event. */
export interface QuickConnectStatus {
  stage: QuickConnectStage;
  mode: QuickConnectMode;
  enabled: boolean;
  relay: { name: string; url: string; accountId?: string } | null;
  /** This computer holds a credential for its relay; `connected` is the live socket. */
  attached: boolean;
  connected: boolean;
  setup: QuickConnectSetup;
  /** `quick_connect_continue` / `quick_connect_choose_account` have something to repeat. */
  canContinue: boolean;
  tools: { source: 'system' | 'downloaded'; nodeVersion: string } | null;
  devices: QuickConnectDevice[];
  /** A pairing code is waiting to be scanned until `expiresAt` (ms). */
  pairing: { expiresAt: number } | null;
  modes: {
    quickConnect: { available: boolean };
    sameWifi: { available: boolean; reason?: string };
    tailscale: { available: boolean; installed: boolean; reason?: string };
  };
  /** The older GitHub-based PandaCode Connect: offered in Settings only when `visible`. */
  legacyConnect: { visible: boolean; signedIn: boolean };
}
/** `quick_connect_pairing_create`: `link` is also the QR code's content. Treat it as a password. */
export interface QuickConnectPairing { link: string; expiresAt: number; scope: QuickConnectScope }
/** The `quick-connect://paired` event. */
export interface QuickConnectPaired { deviceId: string; name: string; scope: QuickConnectScope }
