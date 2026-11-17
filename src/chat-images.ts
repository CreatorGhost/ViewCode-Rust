/* Images in a reply. The Markdown renderer leaves `![alt](src)` as a link (`a.md-image`);
 * here, on the same pass that decorates file links, a link to a local file becomes the image
 * itself. Every Markdown surface with a workspace (the transcript, a streaming reply, subagent
 * and worker panes, the side chat) gets it from this one place.
 *
 * What may load (ViewCode's rule, enforced natively in `chat_images.rs`): a file on this
 * computer, inside or outside the project, that resolves to a regular file with an image
 * extension. A relative path is taken from the task's checkout. Nothing is fetched from the
 * network: an http(s) image stays the link it was. */
export interface ChatImageContext { projectId: string; sessionId?: string }
export interface ChatImage { url: string; name: string; path: string; scaled?: boolean }
export type ChatImageLoader = (context: ChatImageContext, path: string) => Promise<ChatImage>;
export type ImageSource = { kind: 'local'; path: string } | { kind: 'remote'; url: string } | { kind: 'blocked' };

const decode = (value: string): string => { try { return decodeURIComponent(value); } catch { return value; } };
/** `/C:/shots/a.png` (how a drive path survives as a link) back to `C:/shots/a.png`. */
const drive = (path: string): string => /^\/[a-z]:[\\/]/i.test(path) ? path.slice(1) : path;

/** Where a Markdown image's source points. Only `local` is ever loaded. */
export function classifyImageSource(value: string | null | undefined): ImageSource {
  let source = (value ?? '').trim();
  if (source.startsWith('<') && source.endsWith('>')) source = source.slice(1, -1).trim();
  if (!source || source.length > 4096 || source.startsWith('#') || source.startsWith('?')) return { kind: 'blocked' };
  if (/^(?:https?:)?\/\//i.test(source)) return { kind: 'remote', url: source };
  let path: string;
  if (/^file:/i.test(source)) {
    try {
      const url = new URL(source);
      if (url.protocol !== 'file:' || (url.hostname && url.hostname !== 'localhost')) return { kind: 'blocked' };
      path = drive(decode(url.pathname));
    } catch { return { kind: 'blocked' }; }
  } else path = drive(decode(source.split(/[?#]/, 1)[0]));
  if (!path || /[\x00-\x1f]/.test(path)) return { kind: 'blocked' };
  if (path.startsWith('/') || /^[a-z]:[\\/]/i.test(path)) return { kind: 'local', path };
  // Any other scheme (data:, blob:, javascript:) and home-relative paths are never read.
  if (/^[a-z][a-z\d+.-]*:/i.test(path) || /^~[\\/]/.test(path) || path.startsWith('\\\\')) return { kind: 'blocked' };
  return { kind: 'local', path };
}

type Entry = { state: 'loading' | 'ready' | 'failed'; image?: ChatImage; width?: number; height?: number; error?: string; promise: Promise<ChatImage | undefined> };
/** The box an inline image is drawn in; the viewer shows it whole. */
const MAX_WIDTH = 560, MAX_HEIGHT = 360;
/** Loaded images by context and path, so a redraw or a streaming repaint shows them at once.
 * Bounded by count and by the size of the data: URLs it holds. */
const cache = new Map<string, Entry>();
const CACHE_ENTRIES = 48, CACHE_CHARS = 64 * 1024 * 1024;
let loader: ChatImageLoader | undefined;

export function configureChatImages(load: ChatImageLoader): void { loader = load; cache.clear(); }
/** Forget failures (and optionally everything) so files an agent has since written show up. */
export function refreshChatImages(all = false): void {
  for (const [key, entry] of cache) if (all || entry.state === 'failed') cache.delete(key);
  if (typeof document === 'undefined') return;
  for (const frame of document.querySelectorAll<HTMLElement>('.chat-image[data-state="failed"]')) {
    const { imageContext, imagePath } = frame.dataset;
    if (!imageContext || !imagePath) continue;
    try { paint(frame, request(JSON.parse(imageContext) as ChatImageContext, imagePath)); } catch { /* Stays unavailable. */ }
  }
}

const keyOf = (context: ChatImageContext, path: string): string => JSON.stringify([context.projectId, context.sessionId ?? '', path]);

function trim(): void {
  let chars = 0;
  for (const entry of cache.values()) chars += entry.image?.url.length ?? 0;
  for (const [key, entry] of cache) {
    if (cache.size <= CACHE_ENTRIES && chars <= CACHE_CHARS) break;
    if (entry.state === 'loading') continue;
    chars -= entry.image?.url.length ?? 0; cache.delete(key);
  }
}

function request(context: ChatImageContext, path: string): Entry {
  const key = keyOf(context, path), known = cache.get(key);
  if (known) { cache.delete(key); cache.set(key, known); return known; }
  const entry: Entry = { state: 'loading', promise: Promise.resolve(undefined) };
  entry.promise = (loader ? loader(context, path) : Promise.reject(new Error('Images are unavailable here.'))).then(image => {
    if (!/^data:image\/[a-z0-9.+-]+;base64,/i.test(image?.url ?? '')) throw new Error('Image unavailable.');
    // Measured before it is shown, so the frame reserves its size and the transcript never jumps.
    const probe = new Image();
    probe.src = image.url;
    return probe.decode().then(() => {
      const scale = Math.min(1, MAX_WIDTH / (probe.naturalWidth || 1), MAX_HEIGHT / (probe.naturalHeight || 1));
      if (probe.naturalWidth && probe.naturalHeight) { entry.width = Math.max(1, Math.round(probe.naturalWidth * scale)); entry.height = Math.max(1, Math.round(probe.naturalHeight * scale)); }
      entry.state = 'ready'; entry.image = image;
      return image;
    }, () => { throw new Error('This image is damaged or in a format that cannot be shown.'); });
  }).catch((error: unknown) => {
    entry.state = 'failed'; entry.error = error instanceof Error ? error.message : String(error ?? 'Image unavailable.');
    return undefined;
  }).finally(() => { trim(); repaint(key, entry); });
  cache.set(key, entry);
  return entry;
}

/** One image, for callers outside Markdown (the viewer opened from a file link). */
export function loadChatImage(context: ChatImageContext, path: string): Promise<ChatImage> {
  const entry = request(context, path);
  return entry.promise.then(image => { if (!image) throw new Error(entry.error || 'Image unavailable.'); return image; });
}

// Every frame is drawn from its entry alone, so a node prepared for a repaint equals the one
// already on screen and the streaming reconciler leaves the loaded image in place.
function paint(frame: HTMLElement, entry: Entry): void {
  const alt = frame.dataset.imageAlt ?? '';
  if (frame.dataset.state === entry.state && (entry.state !== 'ready' || frame.querySelector('img')?.getAttribute('src') === entry.image!.url)) return;
  frame.dataset.state = entry.state;
  if (entry.state === 'ready') {
    const img = document.createElement('img');
    img.alt = alt; img.decoding = 'async'; img.draggable = false;
    if (entry.width && entry.height) { img.width = entry.width; img.height = entry.height; }
    img.src = entry.image!.url;
    frame.replaceChildren(img);
    frame.removeAttribute('disabled');
    frame.setAttribute('aria-label', `View image: ${alt || entry.image!.name}`);
    frame.title = entry.image!.path;
  } else {
    const label = document.createElement('span');
    label.textContent = entry.state === 'failed' ? `Image unavailable${alt ? `: ${alt}` : ''}` : alt || 'Loading image…';
    frame.replaceChildren(label);
    frame.setAttribute('disabled', '');
    frame.removeAttribute('aria-label');
    if (entry.state === 'failed') frame.title = entry.error ?? ''; else frame.removeAttribute('title');
  }
}

function repaint(key: string, entry: Entry): void {
  for (const frame of document.querySelectorAll<HTMLElement>('.chat-image')) if (frame.dataset.imageKey === key) paint(frame, entry);
}

/** Replace local image links under `root` with the images. Run before file links are
 * decorated (they would turn the same anchors into file references). */
export function decorateChatImages(root: ParentNode, context: ChatImageContext | undefined): void {
  if (!context?.projectId || !loader) return;
  for (const anchor of root.querySelectorAll<HTMLAnchorElement>('a.md-image')) {
    const source = classifyImageSource(anchor.getAttribute('href'));
    if (source.kind !== 'local') continue;
    const frame = document.createElement('button');
    frame.type = 'button'; frame.className = 'chat-image';
    frame.dataset.imageKey = keyOf(context, source.path);
    frame.dataset.imageContext = JSON.stringify({ projectId: context.projectId, ...(context.sessionId ? { sessionId: context.sessionId } : {}) });
    frame.dataset.imagePath = source.path;
    frame.dataset.imageAlt = anchor.textContent === 'Image' ? '' : anchor.textContent ?? '';
    paint(frame, request(context, source.path));
    anchor.replaceWith(frame);
  }
}

export interface GalleryImage { name: string; src: string; path?: string }
/** The loaded images around `frame` (its message, or its Markdown block), and its place among them. */
export function chatImageGallery(frame: HTMLElement): { images: GalleryImage[]; index: number } {
  const scope = frame.closest('.message, .markdown, [data-image-gallery]') ?? frame.parentElement ?? frame;
  const frames = [...scope.querySelectorAll<HTMLElement>('.chat-image[data-state="ready"]')];
  const images = frames.flatMap(item => {
    const img = item.querySelector('img');
    return img?.src ? [{ name: img.alt || item.title.split(/[\\/]/).pop() || 'Image', src: img.src, path: item.title || undefined }] : [];
  });
  return { images, index: Math.max(0, frames.indexOf(frame)) };
}

let listening = false;
/** One click handler for every image in the window. The viewer is its own dialog, so opening
 * it draws nothing in the transcript. */
export function installChatImageClicks(): void {
  if (listening) return;
  listening = true;
  document.addEventListener('click', event => {
    const frame = (event.target as Element | null)?.closest?.<HTMLElement>('.chat-image[data-state="ready"]');
    if (!frame) return;
    event.preventDefault();
    const { images, index } = chatImageGallery(frame);
    // A reply still streaming may redraw the frame; focus then returns to the image that replaced it.
    const again = (): HTMLElement | null => frame.isConnected ? frame : [...document.querySelectorAll<HTMLElement>('.chat-image')].find(other => other.dataset.imageKey === frame.dataset.imageKey) ?? null;
    if (images.length) void import('./image-viewer.ts').then(viewer => viewer.openImageViewer(images, index, again));
  });
}
