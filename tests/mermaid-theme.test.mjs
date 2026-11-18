// Ported from ViewCode's apps/web/src/components/chat/mermaidTheme.test.ts (vitest → node:test),
// plus PandaCode's own default tokens (flat near-black, teal accent).
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  buildMermaidTheme, contrastRatio, defaultClassSetsFill, deriveDiagramPalette, diagramPaletteSignature,
  flattenColor, groupHueCycle, mermaidRenderKey, parseCssColor, planFlowchartInk, planFlowchartTones,
} from '../src/mermaid-theme.ts';

const rgb = hex => {
  const color = parseCssColor(hex);
  if (!color) throw new Error(`bad color ${hex}`);
  return { r: color.r, g: color.g, b: color.b };
};

// ViewCode dark and light, as the document resolves them.
const DARK_TOKENS = {
  canvas: rgb('#19191b'), text: rgb('#dededf'), muted: rgb('#999999'),
  border: flattenColor({ r: 255, g: 255, b: 255, a: 0.08 }, rgb('#19191b')),
  accent: rgb('#e0a15a'), warning: rgb('#c9b037'), danger: rgb('#e5675f'),
};
const LIGHT_TOKENS = {
  canvas: rgb('#ffffff'), text: rgb('#1d1d1f'), muted: rgb('#6e6e73'), border: rgb('#e5e5e7'),
  accent: rgb('#3b6fd9'), warning: rgb('#b7791f'), danger: rgb('#d64545'),
};
// PandaCode's defaults: the diagram canvas is --raised; the accent is the Obsidian teal.
const PANDA_DARK = {
  canvas: rgb('#262626'), text: rgb('#ededed'), muted: rgb('#959595'), border: rgb('#2e2e2e'),
  accent: rgb('#2dd4bf'), warning: rgb('#e3b24f'), danger: rgb('#fa524a'),
};
const PANDA_LIGHT = {
  canvas: rgb('#f1f1ef'), text: rgb('#1f2322'), muted: rgb('#656560'), border: rgb('#e3e3df'),
  accent: rgb('#0f8577'), warning: rgb('#a8700a'), danger: rgb('#b03535'),
};

test('parseCssColor reads the formats getComputedStyle returns', () => {
  assert.deepEqual(parseCssColor('rgb(25, 25, 27)'), { r: 25, g: 25, b: 27, a: 1 });
  assert.deepEqual(parseCssColor('rgba(255, 255, 255, 0.08)'), { r: 255, g: 255, b: 255, a: 0.08 });
  assert.deepEqual(parseCssColor('rgb(255 255 255 / 8%)'), { r: 255, g: 255, b: 255, a: 0.08 });
  assert.deepEqual(parseCssColor('color(srgb 1 0.5 0 / 0.5)'), { r: 255, g: 128, b: 0, a: 0.5 });
  assert.deepEqual(parseCssColor('#E0A15A'), { r: 224, g: 161, b: 90, a: 1 });
  assert.deepEqual(parseCssColor('#fff8'), { r: 255, g: 255, b: 255, a: 136 / 255 });
});

test('parseCssColor leaves other color spaces to the canvas fallback', () => {
  assert.equal(parseCssColor('oklch(0.7 0.1 60)'), null);
  assert.equal(parseCssColor('color(display-p3 1 0 0)'), null);
  assert.equal(parseCssColor('#12345'), null);
});

test('deriveDiagramPalette detects the appearance from the canvas', () => {
  assert.equal(deriveDiagramPalette(DARK_TOKENS, 'Inter').dark, true);
  assert.equal(deriveDiagramPalette(LIGHT_TOKENS, 'Inter').dark, false);
  assert.equal(deriveDiagramPalette(PANDA_DARK, 'Inter').dark, true);
  assert.equal(deriveDiagramPalette(PANDA_LIGHT, 'Inter').dark, false);
});

test('node and group text stay readable in both appearances', () => {
  for (const tokens of [DARK_TOKENS, LIGHT_TOKENS, PANDA_DARK, PANDA_LIGHT]) {
    const palette = deriveDiagramPalette(tokens, 'Inter');
    const text = rgb(palette.text);
    for (const tone of [palette.base, palette.note, ...Object.values(palette.tones)]) {
      assert.ok(contrastRatio(rgb(tone.fill), text) >= 7, `${tone.fill} on ${palette.text}`);
      assert.ok(contrastRatio(rgb(tone.clusterFill), rgb(tone.clusterText)) >= 4.5);
    }
    assert.ok(contrastRatio(rgb(palette.edgeLabel), rgb(palette.muted)) >= 3);
  }
});

test('fills stay soft: closer to the canvas than their strokes', () => {
  for (const tokens of [LIGHT_TOKENS, PANDA_LIGHT, PANDA_DARK]) {
    const palette = deriveDiagramPalette(tokens, 'Inter');
    const canvas = rgb(palette.canvas);
    for (const tone of Object.values(palette.tones)) {
      assert.ok(contrastRatio(rgb(tone.fill), canvas) < contrastRatio(rgb(tone.stroke), canvas));
    }
  }
});

test('the cache key changes when the theme or font changes', () => {
  const source = 'flowchart LR\n  A --> B';
  const dark = diagramPaletteSignature(deriveDiagramPalette(DARK_TOKENS, 'Inter'));
  const light = diagramPaletteSignature(deriveDiagramPalette(LIGHT_TOKENS, 'Inter'));
  const otherFont = diagramPaletteSignature(deriveDiagramPalette(DARK_TOKENS, 'Georgia'));
  assert.notEqual(mermaidRenderKey(source, dark), mermaidRenderKey(source, light));
  assert.notEqual(mermaidRenderKey(source, dark), mermaidRenderKey(source, otherFont));
  assert.equal(mermaidRenderKey(`\n${source}\n\n`, dark), mermaidRenderKey(source, dark));
});

test('groupHueCycle drops the palette hue that would read as the accent', () => {
  assert.ok(!groupHueCycle(rgb('#e0a15a')).includes('amber'));
  assert.ok(!groupHueCycle(rgb('#3b6fd9')).includes('blue'));
  // PandaCode's teal accent: groups never look like the default (accent) node.
  assert.ok(!groupHueCycle(rgb('#2dd4bf')).includes('teal'));
});

test('groupHueCycle keeps every hue for a neutral accent', () => {
  assert.equal(groupHueCycle(rgb('#8a8a8c')).length, 6);
});

test('buildMermaidTheme uses the app font and turns off gradients and shadows', () => {
  const { themeVariables, themeCSS } = buildMermaidTheme(deriveDiagramPalette(DARK_TOKENS, 'Inter, sans-serif'));
  assert.equal(themeVariables.fontFamily, 'Inter, sans-serif');
  assert.equal(themeVariables.darkMode, true);
  assert.equal(themeVariables.useGradient, false);
  assert.equal(themeVariables.dropShadow, 'none');
  assert.ok(themeCSS.includes('.node.vcToneBlue > rect'));
  assert.doesNotMatch(themeCSS, /url\(/);
});

test('charts are themed: pie slices, xychart series and gantt bars come from the palette', () => {
  const palette = deriveDiagramPalette(PANDA_DARK, 'Inter');
  const { themeVariables } = buildMermaidTheme(palette);
  const swatches = Array.from({ length: 6 }, (_, i) => themeVariables[`pie${i + 1}`]);
  assert.equal(new Set(swatches).size, 6, 'six distinct pie slices');
  assert.equal(themeVariables.xyChart.backgroundColor, palette.canvas);
  assert.equal(themeVariables.xyChart.plotColorPalette.split(',').length, 6);
  assert.equal(themeVariables.xyChart.plotColorPalette.split(',')[0], swatches[0]);
  assert.equal(themeVariables.taskBkgColor, palette.base.clusterStroke);
  assert.equal(themeVariables.critBorderColor, palette.danger.stroke);
  // Every colour handed to Mermaid is a plain hex: nothing it could resolve against the network.
  for (const [key, value] of Object.entries(themeVariables)) {
    if (typeof value === 'string' && /color|bkg|fill|pie\d|cScale|git\d/i.test(key)) assert.match(value, /^#[0-9a-f]{6}$|^\d/, key);
  }
});

test('planFlowchartTones gives each group its own tone, innermost group first', () => {
  const cycle = groupHueCycle(rgb('#8a8a8c'));
  assert.deepEqual(planFlowchartTones({
    subgraphs: [
      { id: 'inner', nodes: ['B'], styled: false },
      { id: 'outer', nodes: ['A', 'B', 'inner'], styled: false },
      { id: 'other', nodes: ['C'], styled: false },
    ],
    vertices: [
      { id: 'A', shape: 'square', styled: false },
      { id: 'B', shape: 'square', styled: false },
      { id: 'C', shape: 'square', styled: false },
    ],
  }, cycle), ['class inner,B vcToneBlue', 'class outer,A vcToneAmber', 'class other,C vcToneTeal']);
});

test('planFlowchartTones colours decisions, terminals and stores outside groups, and nothing the author styled', () => {
  const cycle = groupHueCycle(rgb('#8a8a8c'));
  assert.deepEqual(planFlowchartTones({
    subgraphs: [],
    vertices: [
      { id: 'start', shape: 'stadium', styled: false },
      { id: 'check', shape: 'diamond', styled: false },
      { id: 'db', shape: 'cylinder', styled: false },
      { id: 'step', shape: 'square', styled: false },
      { id: 'custom', shape: 'diamond', styled: true },
    ],
  }, cycle), ['class start vcToneGreen', 'class check vcToneAmber', 'class db vcToneViolet']);
});

test('planFlowchartTones falls back to the next role hue when the accent took the first', () => {
  assert.deepEqual(planFlowchartTones({ subgraphs: [], vertices: [{ id: 'check', shape: 'diamond', styled: false }] }, groupHueCycle(rgb('#e0a15a'))), ['class check vcToneViolet']);
});

const dark = deriveDiagramPalette(DARK_TOKENS, 'Inter');
const light = deriveDiagramPalette(LIGHT_TOKENS, 'Inter');
const item = (id, styles = [], classes = []) => ({ id, styles, classes });
const inkFor = (statements, id) => {
  const className = statements.find(line => line.startsWith('class ') && line.split(' ')[1]?.split(',').includes(id))?.split(' ')[2];
  if (!className) return undefined;
  return statements.find(line => line.startsWith(`classDef ${className} `))?.split('color:')[1];
};

test('planFlowchartInk gives light author fills dark text in dark mode, keeping the fill', () => {
  const statements = planFlowchartInk({
    classes: new Map(),
    vertices: [item('pale', ['fill:#fdd']), item('deep', ['fill:#1e3a8a']), item('plain'), item('inked', ['fill:#fdd', 'color:#222'])],
    subgraphs: [],
  }, dark);
  assert.deepEqual(statements, [`classDef vcInk0 color:${dark.canvas}`, 'class pale vcInk0']);
  assert.ok(contrastRatio(rgb(dark.canvas), rgb('#ffdddd')) >= 4.5);
});

test('planFlowchartInk resolves fills and colours through classes, including default, before style lines', () => {
  const classes = new Map([
    ['default', { styles: ['fill:#ffffff'], textStyles: [] }],
    ['warn', { styles: ['fill:#fde68a', 'color:#fef3c7'], textStyles: ['color:#fef3c7'] }],
    ['night', { styles: ['fill:#111111'], textStyles: [] }],
  ]);
  assert.equal(defaultClassSetsFill(classes), true);
  assert.equal(defaultClassSetsFill(new Map([['default', { styles: ['stroke:#f00'] }]])), false);
  const statements = planFlowchartInk({ classes, vertices: [item('A'), item('B', [], ['warn']), item('C', [], ['night']), item('D', ['fill:#111'])], subgraphs: [] }, dark);
  assert.equal(inkFor(statements, 'A'), dark.canvas);
  assert.equal(inkFor(statements, 'B'), dark.canvas);
  assert.equal(inkFor(statements, 'C'), undefined);
  assert.equal(inkFor(statements, 'D'), undefined);
});

test('planFlowchartInk checks subgraph titles against their muted colour and skips their vertex twin', () => {
  assert.deepEqual(planFlowchartInk({ classes: new Map(), vertices: [item('group', ['fill:#333'])], subgraphs: [item('group', ['fill:#333'])] }, light),
    [`classDef vcInk0 color:${light.canvas}`, 'class group vcInk0']);
});

test('planFlowchartInk reads rgb() fills and leaves colours it cannot read to the author', () => {
  const input = { classes: new Map(), vertices: [item('rgb', ['fill:rgb(255, 221, 221) !important']), item('named', ['fill:pink'])], subgraphs: [] };
  const statements = planFlowchartInk(input, dark);
  assert.equal(inkFor(statements, 'rgb'), dark.canvas);
  assert.equal(inkFor(statements, 'named'), undefined);
  const withNames = planFlowchartInk(input, dark, value => (value === 'pink' ? { r: 255, g: 192, b: 203, a: 1 } : parseCssColor(value)));
  assert.equal(inkFor(withNames, 'named'), dark.canvas);
});
