import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyImageSource } from '../src/chat-images.ts';
import { parseMarkdown, renderMarkdown } from '../src/enriched-markdown.ts';

const local = path => ({ kind: 'local', path });

test('a Markdown image source is a local file, a remote link, or blocked', () => {
  // Inside the project (relative to the task's checkout) and anywhere on this computer.
  assert.deepEqual(classifyImageSource('docs/shot.png'), local('docs/shot.png'));
  assert.deepEqual(classifyImageSource('./a%20b.png'), local('./a b.png'));
  assert.deepEqual(classifyImageSource('../sibling/x.png?raw=1#top'), local('../sibling/x.png'));
  assert.deepEqual(classifyImageSource('/tmp/shots/after shot.png'), local('/tmp/shots/after shot.png'));
  assert.deepEqual(classifyImageSource('</tmp/a b.png>'), local('/tmp/a b.png'));
  assert.deepEqual(classifyImageSource('file:///tmp/shots/a%20b.png'), local('/tmp/shots/a b.png'));
  assert.deepEqual(classifyImageSource('file://localhost/tmp/a.png'), local('/tmp/a.png'));
  // Windows drives, as written and as the renderer passes them through the sanitizer.
  assert.deepEqual(classifyImageSource('C:\\shots\\a.png'), local('C:\\shots\\a.png'));
  assert.deepEqual(classifyImageSource('/C:/shots/a.png'), local('C:/shots/a.png'));
  assert.deepEqual(classifyImageSource('file:///C:/shots/a.png'), local('C:/shots/a.png'));
  // A badly encoded name is taken literally rather than refused.
  assert.deepEqual(classifyImageSource('100%.png'), local('100%.png'));

  // The network is never read: these stay links.
  for (const url of ['https://example.com/a.png', 'HTTP://example.com/a.png', '//cdn.example.com/a.png'])
    assert.deepEqual(classifyImageSource(url), { kind: 'remote', url });

  // Nothing else is ever loaded.
  for (const blocked of [null, undefined, '', '   ', '#anchor', '?query', '~/secret.png', '~\\secret.png', 'javascript:alert(1)', 'data:image/png;base64,AAAA',
    'blob:https://example.com/1', 'file://server/share/a.png', 'smb://server/a.png', '\\\\server\\share\\a.png', 'a\nb.png', 'x'.repeat(5000) + '.png'])
    assert.deepEqual(classifyImageSource(blocked), { kind: 'blocked' }, String(blocked));
});

test('the renderer leaves an image as a marked link and never emits an <img>', async () => {
  const render = async value => renderMarkdown(await parseMarkdown(value));
  assert.equal(await render('![Before](docs/before.png)'), '<p dir="auto"><a class="md-media md-image" href="docs/before.png">Before</a></p>\n');
  assert.equal(await render('![](</tmp/after shot.png> "Shot")'), '<p dir="auto"><a class="md-media md-image" href="/tmp/after shot.png">Shot</a></p>\n');
  assert.equal(await render('![](x.png)'), '<p dir="auto"><a class="md-media md-image" href="x.png">Image</a></p>\n');
  // A drive path gets a leading slash, so the sanitizer does not drop `C:` as an unknown scheme.
  assert.match(await render('![Win](<C:\\shots\\a b.png>)'), /href="\/C:\/shots\/a b\.png"/);
  assert.match(await render('![Remote](https://example.com/a.png)'), /<a class="md-media md-image" href="https:\/\/example\.com\/a\.png">Remote<\/a>/);
  assert.doesNotMatch(await render('![a](x.png) <img src="y.png">'), /<img/);
  assert.match(await render('![x"><script>alert(1)</script>](a.png)'), /^<p dir="auto"><a class="md-media md-image" href="a\.png">x"&gt;&lt;script&gt;/);
});
