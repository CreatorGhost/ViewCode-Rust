import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Project colours (src/project-colors.ts): the picker's swatches, the tone attributes and the "colour the app" preference.
const store = new Map(), events = [];
globalThis.localStorage = { getItem: key => store.get(key) ?? null, setItem: (key, value) => { store.set(key, String(value)); } };
globalThis.window = { dispatchEvent: event => { events.push(event.type); return true; } };
const colors = await import('../src/project-colors.ts');
const { PROJECT_COLORS } = await import('../src/palette-core.ts');

test('the picker offers None, then the 18 colours in ProjectColor order, each named', () => {
  const types = readFileSync(new URL('../src/types.ts', import.meta.url), 'utf8');
  const names = types.match(/export type ProjectColor = ([^;]+);/)[1].split('|').map(name => name.trim().replace(/'/g, ''));
  const choices = colors.projectColorChoices(null);
  assert.deepEqual(choices.map(choice => choice.id), ['none', ...names]);
  assert.deepEqual(Object.keys(colors.PROJECT_COLOR_LABELS), names);
  assert.equal(choices.length, 19);
  for (const choice of choices) {
    assert.ok(choice.label.length > 2, choice.id);
    assert.equal(choice.segment, 'Colour');
    // A swatch is a token, never a literal colour; None has no fill.
    assert.equal(choice.swatch, choice.id === 'none' ? '' : `var(--project-${choice.id})`);
  }
  assert.equal(new Set(choices.map(choice => choice.label)).size, 19);
  assert.deepEqual(choices.filter(choice => choice.selected).map(choice => choice.id), ['none']);
});

test('the current colour is the one selected, and colours other projects wear say so', () => {
  const choices = colors.projectColorChoices('violet', ['red', 'violet']);
  assert.deepEqual(choices.filter(choice => choice.selected).map(choice => choice.label), ['Violet']);
  assert.equal(choices.find(choice => choice.id === 'red').label, 'Red (used by another project)');
  assert.equal(choices.find(choice => choice.id === 'blue').label, 'Blue');
  // A name from a newer build is no colour here.
  assert.deepEqual(colors.projectColorChoices('chartreuse').filter(choice => choice.selected).map(choice => choice.id), ['none']);
});

test('a picked swatch maps to what set_project_color takes', () => {
  assert.equal(colors.pickedProjectColor('none'), null);
  for (const name of PROJECT_COLORS) assert.equal(colors.pickedProjectColor(name), name);
  for (const other of ['new', 'reveal', 'remove', '', 'Red', '#ff0000']) assert.equal(colors.pickedProjectColor(other), undefined);
});

test('tone attributes name the token for a known colour and nothing else', () => {
  assert.equal(colors.projectToneAttributes('teal', '--tile-tone'), ' data-color="teal" style="--tile-tone:var(--project-teal)"');
  assert.equal(colors.projectTone('rose'), 'var(--project-rose)');
  for (const none of [null, undefined, '', '#ff0000', 'red" onload="x', 'RED']) {
    assert.equal(colors.projectToneAttributes(none, '--tile-tone'), '');
    assert.equal(colors.projectTone(none), '');
    assert.equal(colors.knownProjectColor(none), '');
  }
});

test('colouring the app by the active project is on by default, saved, and announced', () => {
  assert.equal(colors.projectTintEnabled(), true);
  assert.equal(colors.activeProjectTint('violet'), 'violet');
  assert.equal(colors.activeProjectTint(null), '');
  colors.setProjectTintEnabled(false);
  assert.equal(store.get('pandacode.projectTint'), 'off');
  assert.equal(colors.projectTintEnabled(), false);
  assert.equal(colors.activeProjectTint('violet'), '', 'off: the accent everywhere');
  colors.setProjectTintEnabled(true);
  assert.equal(colors.activeProjectTint('violet'), 'violet');
  assert.deepEqual(events, [colors.TINT_EVENT, colors.TINT_EVENT]);
});
