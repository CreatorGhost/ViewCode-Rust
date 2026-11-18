import { escape, icon } from './dom';

/* The image viewer: one image at a time, fitted to the window, over a scrim. A modal
 * <dialog> of its own, appended to the body, so opening and closing it never redraws what is
 * behind it. Esc, the close button or a click outside the image closes it and focus returns
 * to what opened it; with several images, arrows and the side buttons step through them;
 * a click on the image switches between fitted and full size. */
export interface ViewerImage { name: string; src: string; path?: string }

let current: { close(): void } | undefined;

/** `opener` gets focus back on close; pass a function when the element may be redrawn while the viewer is open. */
export function openImageViewer(images: ViewerImage[], index = 0, opener: HTMLElement | null | (() => HTMLElement | null) = document.activeElement instanceof HTMLElement ? document.activeElement : null): void {
  if (!images.length) return;
  current?.close();
  const many = images.length > 1;
  const dialog = document.createElement('dialog');
  dialog.className = 'image-viewer';
  dialog.innerHTML = `<button type="button" class="icon-button image-viewer-close" data-viewer="close" aria-label="Close image preview" title="Close (Esc)">${icon('close')}</button>
    ${many ? `<button type="button" class="icon-button image-viewer-nav" data-viewer="previous" aria-label="Previous image">${icon('back')}</button>` : ''}
    <figure class="image-viewer-stage"><img alt="" draggable="false"><p class="image-viewer-error" role="alert" hidden>Image unavailable. The file may have been moved or deleted.</p></figure>
    ${many ? `<button type="button" class="icon-button image-viewer-nav" data-viewer="next" aria-label="Next image">${icon('forward')}</button>` : ''}
    <p class="image-viewer-caption" aria-live="polite" aria-atomic="true"></p>`;
  const stage = dialog.querySelector<HTMLElement>('.image-viewer-stage')!, img = stage.querySelector('img')!;
  const failure = stage.querySelector<HTMLElement>('.image-viewer-error')!, caption = dialog.querySelector<HTMLElement>('.image-viewer-caption')!;
  let at = ((index % images.length) + images.length) % images.length, closed = false;

  function show(): void {
    const item = images[at];
    stage.classList.remove('is-zoomed'); failure.hidden = true; img.hidden = false;
    img.alt = item.name; img.src = item.src;
    caption.innerHTML = `<span title="${escape(item.path ?? item.name)}">${escape(item.name)}</span>${many ? ` <span class="image-viewer-count">${at + 1} / ${images.length}</span>` : ''}`;
    dialog.setAttribute('aria-label', many ? `Image preview: ${item.name}, ${at + 1} of ${images.length}` : `Image preview: ${item.name}`);
  }
  function step(by: number): void { if (many) { at = (at + by + images.length) % images.length; show(); } }
  function close(): void {
    if (closed) return;
    closed = true; current = undefined;
    removeEventListener('keydown', onKey, true);
    dialog.close(); dialog.remove();
    const target = typeof opener === 'function' ? opener() : opener;
    if (target?.isConnected) target.focus({ preventScroll: true });
  }
  // Captured on the window: no shortcut of the app behind the viewer acts while it is open.
  function onKey(event: KeyboardEvent): void {
    if (event.key === 'Tab') return;
    event.stopImmediatePropagation();
    if (event.key === 'Escape') { event.preventDefault(); close(); }
    else if (event.key === 'ArrowLeft') { event.preventDefault(); step(-1); }
    else if (event.key === 'ArrowRight') { event.preventDefault(); step(1); }
  }
  img.addEventListener('error', () => { img.hidden = true; failure.hidden = false; });
  dialog.addEventListener('cancel', event => { event.preventDefault(); close(); });
  dialog.addEventListener('click', event => {
    const target = event.target as Element, action = target.closest<HTMLElement>('[data-viewer]')?.dataset.viewer;
    if (action === 'close') close();
    else if (action === 'previous') step(-1);
    else if (action === 'next') step(1);
    // Full size only helps when the image is larger than it is drawn.
    else if (target === img) { if (stage.classList.contains('is-zoomed') || img.naturalWidth > img.clientWidth + 1 || img.naturalHeight > img.clientHeight + 1) stage.classList.toggle('is-zoomed'); }
    else if (target === dialog || target === stage) close();
  });
  document.body.append(dialog);
  show();
  addEventListener('keydown', onKey, true);
  dialog.showModal();
  dialog.querySelector<HTMLElement>('.image-viewer-close')!.focus({ preventScroll: true });
  current = { close };
}

export function closeImageViewer(): void { current?.close(); }
