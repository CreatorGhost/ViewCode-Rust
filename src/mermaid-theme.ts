/* Diagram look for Mermaid, derived from the live theme tokens. Ported from ViewCode
 * (apps/web/src/components/chat/mermaidTheme.ts) with PandaCode's token names.
 *
 * Pure: mermaid-render.ts reads the tokens from the document and hands them here, so Node
 * tests can run it. Fills are soft tints of a small categorical palette mixed into the
 * canvas, so the same rules read well on light, dark, every accent and imported VS Code
 * themes. Class names keep ViewCode's `vcTone*` / `vcInk*` spelling. */

export interface Rgb {
  readonly r: number;
  readonly g: number;
  readonly b: number;
}

export interface Rgba extends Rgb {
  readonly a: number;
}

/** Theme tokens a diagram is drawn from, already flattened onto the canvas. */
export interface DiagramTokens {
  readonly canvas: Rgb;
  readonly text: Rgb;
  readonly muted: Rgb;
  readonly border: Rgb;
  readonly accent: Rgb;
  readonly warning: Rgb;
  readonly danger: Rgb;
}

const DIAGRAM_HUES = ["blue", "violet", "teal", "amber", "rose", "green"] as const;
export type DiagramHue = (typeof DIAGRAM_HUES)[number];

export interface DiagramTone {
  readonly fill: string;
  readonly stroke: string;
  /** Saturated swatch for charts (pie slices, git branches, plot series). */
  readonly strong: string;
  readonly clusterFill: string;
  readonly clusterStroke: string;
  readonly clusterText: string;
}

export interface DiagramPalette {
  readonly dark: boolean;
  readonly fontFamily: string;
  readonly canvas: string;
  readonly text: string;
  readonly muted: string;
  readonly line: string;
  readonly border: string;
  readonly subtle: string;
  readonly edgeLabel: string;
  readonly base: DiagramTone;
  readonly note: DiagramTone;
  readonly danger: DiagramTone;
  readonly tones: Readonly<Record<DiagramHue, DiagramTone>>;
  /** Hues groups cycle through, minus any too close to the theme accent. */
  readonly cycle: ReadonlyArray<DiagramHue>;
}

// Mid-saturation sources; every use mixes them toward the canvas.
const HUE_SOURCES: Record<DiagramHue, Rgb> = {
  blue: { r: 0x3b, g: 0x82, b: 0xf6 },
  violet: { r: 0x8b, g: 0x5c, b: 0xf6 },
  teal: { r: 0x14, g: 0xb8, b: 0xa6 },
  amber: { r: 0xf5, g: 0x9e, b: 0x0b },
  rose: { r: 0xf4, g: 0x3f, b: 0x5e },
  green: { r: 0x22, g: 0xc5, b: 0x5e },
};

// Adjacent groups should not read as the same family.
const GROUP_ORDER: ReadonlyArray<DiagramHue> = ["blue", "amber", "teal", "rose", "violet", "green"];
const ACCENT_HUE_DISTANCE = 28;

const clampByte = (value: number) => Math.min(255, Math.max(0, Math.round(value)));

function parseChannel(token: string, scale: number): number | null {
  const percent = token.endsWith("%");
  const value = Number.parseFloat(token);
  if (!Number.isFinite(value)) return null;
  return percent ? (value / 100) * scale : value;
}

/**
 * Reads the color formats getComputedStyle hands back: hex, rgb()/rgba() in
 * either syntax, and color(srgb …). Other spaces (oklch, lab) return null and
 * the caller resolves them through a canvas.
 */
export function parseCssColor(value: string): Rgba | null {
  const input = value.trim().toLowerCase();
  const hex = /^#([0-9a-f]{3,8})$/.exec(input)?.[1];
  if (hex) {
    if (![3, 4, 6, 8].includes(hex.length)) return null;
    const full = hex.length <= 4 ? [...hex].map((digit) => digit + digit).join("") : hex;
    const [r = 0, g = 0, b = 0, a = 255] = (full.match(/../g) ?? []).map((pair) =>
      Number.parseInt(pair, 16),
    );
    return { r, g, b, a: a / 255 };
  }

  const functional = /^(rgba?|color)\((.*)\)$/.exec(input);
  if (!functional) return null;
  const [, name, body = ""] = functional;
  const [channelPart = "", alphaPart] = body.includes("/") ? body.split("/") : [body, undefined];
  const parts = channelPart.split(/[\s,]+/).filter(Boolean);
  let alphaToken = alphaPart?.trim();
  if (name === "color") {
    if (parts.shift() !== "srgb") return null;
  } else if (alphaToken === undefined && parts.length === 4) {
    alphaToken = parts.pop();
  }
  if (parts.length !== 3) return null;
  const scale = name === "color" ? 1 : 255;
  const channels = parts.map((part) => parseChannel(part, scale));
  const alpha = alphaToken === undefined ? 1 : parseChannel(alphaToken, 1);
  if (channels.some((channel) => channel === null) || alpha === null) return null;
  const [r = 0, g = 0, b = 0] = channels.map((channel) => (channel ?? 0) * (255 / scale));
  return { r: clampByte(r), g: clampByte(g), b: clampByte(b), a: Math.min(1, Math.max(0, alpha)) };
}

/** Composites a translucent token over the surface it is drawn on. */
export function flattenColor(color: Rgba, base: Rgb): Rgb {
  return mixColors(color, base, color.a);
}

/** `weight` of `a`, the rest of `b`, in sRGB like CSS color-mix(in srgb). */
function mixColors(a: Rgb, b: Rgb, weight: number): Rgb {
  return {
    r: clampByte(a.r * weight + b.r * (1 - weight)),
    g: clampByte(a.g * weight + b.g * (1 - weight)),
    b: clampByte(a.b * weight + b.b * (1 - weight)),
  };
}

function toHex(color: Rgb): string {
  return `#${[color.r, color.g, color.b].map((channel) => clampByte(channel).toString(16).padStart(2, "0")).join("")}`;
}

function relativeLuminance(color: Rgb): number {
  const linear = (channel: number) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
}

export function contrastRatio(a: Rgb, b: Rgb): number {
  const [light, dark] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return ((light ?? 0) + 0.05) / ((dark ?? 0) + 0.05);
}

/** Hue in degrees and chroma in 0..1, or null hue for greys. */
function hueOf(color: Rgb): { hue: number | null; chroma: number } {
  const r = color.r / 255;
  const g = color.g / 255;
  const b = color.b / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const chroma = max - min;
  if (chroma < 0.0001) return { hue: null, chroma };
  const sector =
    max === r ? ((g - b) / chroma) % 6 : max === g ? (b - r) / chroma + 2 : (r - g) / chroma + 4;
  return { hue: (sector * 60 + 360) % 360, chroma };
}

function hueDistance(a: number, b: number): number {
  const distance = Math.abs(a - b) % 360;
  return distance > 180 ? 360 - distance : distance;
}

/** Hues far enough from a saturated accent that a group never looks like the default node. */
export function groupHueCycle(accent: Rgb): ReadonlyArray<DiagramHue> {
  const { hue, chroma } = hueOf(accent);
  if (hue === null || chroma < 0.15) return GROUP_ORDER;
  return GROUP_ORDER.filter((name) => {
    const source = hueOf(HUE_SOURCES[name]).hue;
    return source === null || hueDistance(source, hue) >= ACCENT_HUE_DISTANCE;
  });
}

function readableOn(background: Rgb, preferred: Rgb, fallback: Rgb): Rgb {
  return contrastRatio(background, preferred) >= 4.5 ? preferred : fallback;
}

function deriveTone(source: Rgb, tokens: DiagramTokens, dark: boolean): DiagramTone {
  const { canvas, text } = tokens;
  const clusterFill = mixColors(source, canvas, dark ? 0.07 : 0.045);
  return {
    fill: toHex(mixColors(source, canvas, dark ? 0.2 : 0.12)),
    stroke: toHex(mixColors(source, canvas, dark ? 0.62 : 0.55)),
    strong: toHex(mixColors(source, canvas, dark ? 0.82 : 0.88)),
    clusterFill: toHex(clusterFill),
    clusterStroke: toHex(mixColors(source, canvas, dark ? 0.32 : 0.28)),
    clusterText: toHex(readableOn(clusterFill, mixColors(source, text, 0.5), text)),
  };
}

export function deriveDiagramPalette(tokens: DiagramTokens, fontFamily: string): DiagramPalette {
  const { canvas, text, muted } = tokens;
  const dark = relativeLuminance(canvas) < 0.3;
  const tones = Object.fromEntries(
    DIAGRAM_HUES.map((name) => [name, deriveTone(HUE_SOURCES[name], tokens, dark)]),
  ) as Record<DiagramHue, DiagramTone>;
  return {
    dark,
    fontFamily,
    canvas: toHex(canvas),
    text: toHex(text),
    muted: toHex(readableOn(canvas, muted, text)),
    line: toHex(mixColors(text, canvas, dark ? 0.5 : 0.45)),
    border: toHex(mixColors(text, canvas, dark ? 0.22 : 0.18)),
    subtle: toHex(mixColors(text, canvas, dark ? 0.05 : 0.035)),
    edgeLabel: toHex(mixColors(text, canvas, dark ? 0.09 : 0.05)),
    base: deriveTone(tokens.accent, tokens, dark),
    note: deriveTone(tokens.warning, tokens, dark),
    danger: deriveTone(tokens.danger, tokens, dark),
    tones,
    cycle: groupHueCycle(tokens.accent),
  };
}

/** Changes whenever anything that reaches the rendered SVG changes. */
export function diagramPaletteSignature(palette: DiagramPalette): string {
  const tone = (value: DiagramTone) =>
    [
      value.fill,
      value.stroke,
      value.strong,
      value.clusterFill,
      value.clusterStroke,
      value.clusterText,
    ].join(",");
  return [
    palette.fontFamily,
    palette.canvas,
    palette.text,
    palette.muted,
    palette.line,
    palette.border,
    palette.subtle,
    palette.edgeLabel,
    tone(palette.base),
    tone(palette.note),
    tone(palette.danger),
    ...DIAGRAM_HUES.map((name) => tone(palette.tones[name])),
    palette.cycle.join(","),
  ].join("|");
}

/** Cache key for one rendered diagram: the theme it was drawn in plus its source. */
export function mermaidRenderKey(source: string, paletteSignature: string): string {
  return `${paletteSignature}\n${source.trim()}`;
}

const toneClass = (name: DiagramHue) => `vcTone${name[0]?.toUpperCase()}${name.slice(1)}`;

const NODE_SHAPES = ["rect", "polygon", "path", "circle", "ellipse"];

function toneCss(name: DiagramHue, tone: DiagramTone): string {
  const className = toneClass(name);
  const shapes = NODE_SHAPES.flatMap((shape) => [
    `.node.${className} > ${shape}`,
    `.node.${className} > g > ${shape}`,
  ]).join(", ");
  return `${shapes} { fill: ${tone.fill}; stroke: ${tone.stroke}; }
.cluster.${className} > rect { fill: ${tone.clusterFill}; stroke: ${tone.clusterStroke}; }
.cluster.${className} .cluster-label text, .cluster.${className} > g > text { fill: ${tone.clusterText}; }`;
}

/** Mermaid's `base` theme plus the CSS that gives diagrams PandaCode's look. */
export function buildMermaidTheme(palette: DiagramPalette): {
  themeVariables: Record<string, unknown>;
  themeCSS: string;
} {
  const { base, note, danger, tones, text, canvas, muted, line, border, subtle } = palette;
  const scale = [...palette.cycle, ...DIAGRAM_HUES.filter((name) => !palette.cycle.includes(name))];
  const scaleTone = (index: number) => tones[scale[index % scale.length] ?? "blue"];
  const indexed = (prefix: string, count: number, value: (index: number) => string) =>
    Object.fromEntries(Array.from({ length: count }, (_, index) => [prefix + index, value(index)]));
  const strongLabel = (index: number) => {
    const swatch = parseCssColor(scaleTone(index).strong);
    const ink = parseCssColor(text);
    const paper = parseCssColor(canvas);
    if (!swatch || !ink || !paper) return text;
    return contrastRatio(swatch, ink) >= contrastRatio(swatch, paper) ? text : canvas;
  };

  const themeVariables: Record<string, unknown> = {
    darkMode: palette.dark,
    background: canvas,
    fontFamily: palette.fontFamily,
    fontSize: "14px",
    useGradient: false,
    dropShadow: "none",
    strokeWidth: 1.5,
    radius: 8,

    primaryColor: base.fill,
    primaryBorderColor: base.stroke,
    primaryTextColor: text,
    secondaryColor: scaleTone(0).fill,
    secondaryBorderColor: scaleTone(0).stroke,
    secondaryTextColor: text,
    tertiaryColor: subtle,
    tertiaryBorderColor: border,
    tertiaryTextColor: text,
    mainBkg: base.fill,
    nodeBkg: base.fill,
    nodeBorder: base.stroke,
    nodeTextColor: text,
    textColor: text,
    titleColor: text,
    lineColor: line,
    arrowheadColor: line,
    defaultLinkColor: line,
    border1: border,
    border2: border,
    clusterBkg: subtle,
    clusterBorder: border,
    edgeLabelBackground: palette.edgeLabel,
    labelBackgroundColor: palette.edgeLabel,

    noteBkgColor: note.fill,
    noteBorderColor: note.stroke,
    noteTextColor: text,
    actorBkg: base.fill,
    actorBorder: base.stroke,
    actorTextColor: text,
    actorLineColor: border,
    signalColor: line,
    signalTextColor: text,
    labelBoxBkgColor: base.fill,
    labelBoxBorderColor: base.stroke,
    labelTextColor: text,
    loopTextColor: muted,
    activationBkgColor: base.clusterStroke,
    activationBorderColor: base.stroke,
    sequenceNumberColor: canvas,

    classText: text,
    stateBkg: base.fill,
    stateLabelColor: text,
    compositeBackground: subtle,
    compositeTitleBackground: base.fill,
    compositeBorder: border,
    altBackground: subtle,
    transitionColor: line,
    transitionLabelColor: muted,
    specialStateColor: line,
    innerEndBackground: line,
    attributeBackgroundColorOdd: canvas,
    attributeBackgroundColorEven: subtle,
    relationColor: line,
    relationLabelBackground: palette.edgeLabel,
    relationLabelColor: text,
    requirementBackground: base.fill,
    requirementBorderColor: base.stroke,
    requirementTextColor: text,
    errorBkgColor: danger.fill,
    errorTextColor: text,

    sectionBkgColor: base.clusterFill,
    altSectionBkgColor: canvas,
    sectionBkgColor2: scaleTone(0).clusterFill,
    taskBkgColor: base.clusterStroke,
    taskBorderColor: base.stroke,
    taskTextColor: text,
    taskTextLightColor: text,
    taskTextDarkColor: text,
    taskTextOutsideColor: text,
    activeTaskBkgColor: base.fill,
    activeTaskBorderColor: base.stroke,
    doneTaskBkgColor: subtle,
    doneTaskBorderColor: border,
    critBkgColor: danger.clusterStroke,
    critBorderColor: danger.stroke,
    todayLineColor: danger.stroke,
    vertLineColor: line,
    gridColor: border,
    excludeBkgColor: subtle,

    ...indexed("cScale", 12, (index) => scaleTone(index).fill),
    ...indexed("cScalePeer", 12, (index) => scaleTone(index).stroke),
    ...indexed("cScaleInv", 12, () => text),
    ...indexed("cScaleLabel", 12, () => text),
    ...indexed("fillType", 8, (index) => scaleTone(index).fill),
    ...Object.fromEntries(
      Array.from({ length: 12 }, (_, index) => [`pie${index + 1}`, scaleTone(index).strong]),
    ),
    pieStrokeColor: canvas,
    pieStrokeWidth: "1.5px",
    pieOuterStrokeColor: border,
    pieOuterStrokeWidth: "1px",
    pieOpacity: "1",
    pieTitleTextSize: "16px",
    pieTitleTextColor: text,
    pieSectionTextSize: "13px",
    pieSectionTextColor: strongLabel(0),
    pieLegendTextSize: "13px",
    pieLegendTextColor: text,
    ...indexed("git", 8, (index) => scaleTone(index).strong),
    ...indexed("gitInv", 8, (index) => strongLabel(index)),
    ...indexed("gitBranchLabel", 8, (index) => strongLabel(index)),
    commitLabelColor: text,
    commitLabelBackground: palette.edgeLabel,
    tagLabelColor: text,
    tagLabelBackground: base.fill,
    tagLabelBorder: base.stroke,
    quadrant1Fill: scaleTone(0).clusterFill,
    quadrant2Fill: scaleTone(1).clusterFill,
    quadrant3Fill: scaleTone(2).clusterFill,
    quadrant4Fill: scaleTone(3).clusterFill,
    quadrant1TextFill: text,
    quadrant2TextFill: text,
    quadrant3TextFill: text,
    quadrant4TextFill: text,
    quadrantPointFill: base.strong,
    quadrantPointTextFill: text,
    quadrantXAxisTextFill: text,
    quadrantYAxisTextFill: text,
    quadrantTitleFill: text,
    quadrantInternalBorderStrokeFill: border,
    quadrantExternalBorderStrokeFill: border,
    // Replaces Mermaid's whole xyChart object, so every key is spelled out.
    xyChart: {
      backgroundColor: canvas,
      titleColor: text,
      dataLabelColor: text,
      legendTextColor: text,
      xAxisLabelColor: muted,
      xAxisTitleColor: text,
      xAxisTickColor: border,
      xAxisLineColor: border,
      yAxisLabelColor: muted,
      yAxisTitleColor: text,
      yAxisTickColor: border,
      yAxisLineColor: border,
      plotColorPalette: Array.from({ length: 6 }, (_, index) => scaleTone(index).strong).join(","),
    },
  };

  // Geometry the layout already measured (font size, padding) stays in the
  // config; this only repaints and rounds what Mermaid drew.
  const themeCSS = `
.node > rect.label-container, .node > rect.basic { rx: 8px; ry: 8px; }
.cluster > rect { rx: 12px; ry: 12px; stroke-width: 1px; }
.cluster-label text, .cluster > g > text { fill: ${muted}; }
.flowchart-link, .edgePaths .path { stroke-width: 1.5px; stroke-linecap: round; stroke-linejoin: round; }
.edgeLabel rect, .edgeLabel .background { opacity: 1; fill: ${palette.edgeLabel}; }
.edgeLabel text, .edgeLabel .label text { fill: ${muted}; }
rect.actor { rx: 8px; ry: 8px; }
rect.note { rx: 6px; ry: 6px; }
.actor-line { stroke-dasharray: 4 4; }
${DIAGRAM_HUES.map((name) => toneCss(name, tones[name])).join("\n")}
`;

  return { themeVariables, themeCSS };
}

export interface FlowchartVertex {
  readonly id: string;
  readonly shape: string | undefined;
  /** Carries the author's own `style`, `class` or `:::` styling. */
  readonly styled: boolean;
}

export interface FlowchartSubgraph {
  readonly id: string;
  readonly nodes: ReadonlyArray<string>;
  readonly styled: boolean;
}

const SHAPE_ROLES: ReadonlyArray<{
  readonly shapes: ReadonlySet<string>;
  readonly hues: ReadonlyArray<DiagramHue>;
}> = [
  {
    // Decisions
    shapes: new Set(["diamond", "diam", "decision", "question", "hexagon", "hex", "prepare"]),
    hues: ["amber", "violet"],
  },
  {
    // Start and end
    shapes: new Set([
      "stadium",
      "pill",
      "terminal",
      "circle",
      "circ",
      "doublecircle",
      "dbl-circ",
      "double-circle",
      "sm-circ",
      "small-circle",
      "start",
      "fr-circ",
      "framed-circle",
      "stop",
    ]),
    hues: ["green", "teal"],
  },
  {
    // Data stores
    shapes: new Set(["cylinder", "cyl", "db", "database", "h-cyl", "das", "lin-cyl", "disk"]),
    hues: ["violet", "blue"],
  },
];

/**
 * Picks a tone per subgraph (cycling through the palette, innermost group wins)
 * and per semantic shape outside groups, then spells it as Mermaid `class`
 * statements to append to the source. Anything the author styled keeps its look.
 */
export function planFlowchartTones(
  input: {
    readonly subgraphs: ReadonlyArray<FlowchartSubgraph>;
    readonly vertices: ReadonlyArray<FlowchartVertex>;
  },
  cycle: ReadonlyArray<DiagramHue>,
): string[] {
  if (cycle.length === 0) return [];
  const assignments = new Map<DiagramHue, string[]>();
  const assign = (id: string, hue: DiagramHue) => {
    const ids = assignments.get(hue) ?? [];
    ids.push(id);
    assignments.set(hue, ids);
  };

  const groupHue = new Map<string, DiagramHue>();
  input.subgraphs.forEach((subgraph, index) => {
    const hue = cycle[index % cycle.length];
    if (!hue) return;
    groupHue.set(subgraph.id, hue);
    if (!subgraph.styled) assign(subgraph.id, hue);
  });

  const subgraphIds = new Set(input.subgraphs.map((subgraph) => subgraph.id));
  for (const vertex of input.vertices) {
    if (vertex.styled || subgraphIds.has(vertex.id)) continue;
    const innermost = input.subgraphs
      .filter((subgraph) => subgraph.nodes.includes(vertex.id))
      .sort((a, b) => a.nodes.length - b.nodes.length)[0];
    const hue = innermost
      ? groupHue.get(innermost.id)
      : SHAPE_ROLES.find(
          (role) => vertex.shape !== undefined && role.shapes.has(vertex.shape),
        )?.hues.find((candidate) => cycle.includes(candidate));
    if (hue) assign(vertex.id, hue);
  }

  return [...assignments].map(([hue, ids]) => `class ${ids.join(",")} ${toneClass(hue)}`);
}

/** A flowchart `classDef`, as Mermaid's flow db keeps it. */
export interface FlowchartClassDef {
  readonly styles: ReadonlyArray<string>;
  readonly textStyles?: ReadonlyArray<string> | undefined;
}

/** A node or subgraph with the author's own `style` declarations and classes. */
export interface FlowchartStyledItem {
  readonly id: string;
  readonly styles: ReadonlyArray<string>;
  readonly classes: ReadonlyArray<string>;
}

const BLACK: Rgb = { r: 0, g: 0, b: 0 };
const WHITE: Rgb = { r: 255, g: 255, b: 255 };

/** The last value a list of `key:value` declarations gives `key`, as Mermaid resolves it. */
function declaredValue(declarations: ReadonlyArray<string>, key: string): string | undefined {
  let value: string | undefined;
  for (const declaration of declarations) {
    const separator = declaration.indexOf(":");
    if (separator < 0 || declaration.slice(0, separator).trim().toLowerCase() !== key) continue;
    value = declaration
      .slice(separator + 1)
      .replace(/!important/i, "")
      .trim();
  }
  return value;
}

/**
 * Whether `classDef default` fills every node. Its rule is `!important`, so
 * palette tones would only recolour the strokes.
 */
export function defaultClassSetsFill(classes: ReadonlyMap<string, FlowchartClassDef>): boolean {
  return declaredValue(classes.get("default")?.styles ?? [], "fill") !== undefined;
}

/**
 * Author colours win over the theme, but a `fill` without a `color` keeps the
 * theme's text colour, which is near-white in dark mode. For every node and
 * subgraph whose effective fill leaves its text below 4.5:1, this returns
 * Mermaid statements that give it readable text: a `classDef` per ink plus
 * `class` lines. A class is used rather than `style`, because Mermaid turns
 * its colour into a `tspan` rule, which beats a colour the author set inline
 * or through another class, and is the only colour that reaches subgraph
 * titles when labels are SVG text.
 *
 * `parseColor` reads a CSS colour, or returns null; items whose colours it
 * can't read keep the author's look.
 */
export function planFlowchartInk(
  input: {
    readonly classes: ReadonlyMap<string, FlowchartClassDef>;
    readonly vertices: ReadonlyArray<FlowchartStyledItem>;
    readonly subgraphs: ReadonlyArray<FlowchartStyledItem>;
  },
  palette: Pick<DiagramPalette, "canvas" | "text" | "muted">,
  parseColor: (value: string) => Rgba | null = parseCssColor,
): string[] {
  const canvas = parseCssColor(palette.canvas);
  const text = parseCssColor(palette.text);
  if (!canvas || !text) return [];
  // Mermaid's own order: each class's styles then its text styles, then `style` lines.
  const declarations = (item: FlowchartStyledItem, classNames: ReadonlyArray<string>) => [
    ...classNames.flatMap((name) => {
      const classDef = input.classes.get(name);
      return classDef ? [...classDef.styles, ...(classDef.textStyles ?? [])] : [];
    }),
    ...item.styles,
  ];

  const inks = new Map<string, string[]>();
  const check = (id: string, styles: ReadonlyArray<string>, themeText: string) => {
    const fillValue = declaredValue(styles, "fill");
    const fill = fillValue === undefined ? null : parseColor(fillValue);
    if (!fill) return;
    const surface = flattenColor(fill, canvas);
    const colorValue = declaredValue(styles, "color");
    const current = colorValue === undefined ? parseCssColor(themeText) : parseColor(colorValue);
    if (!current || contrastRatio(surface, flattenColor(current, surface)) >= 4.5) return;
    const extreme = contrastRatio(surface, BLACK) >= contrastRatio(surface, WHITE) ? BLACK : WHITE;
    const ink = toHex(readableOn(surface, text, readableOn(surface, canvas, extreme)));
    inks.set(ink, [...(inks.get(ink) ?? []), id]);
  };

  const subgraphIds = new Set(input.subgraphs.map((subgraph) => subgraph.id));
  for (const vertex of input.vertices) {
    if (subgraphIds.has(vertex.id)) continue;
    check(vertex.id, declarations(vertex, ["default", ...vertex.classes]), palette.text);
  }
  // Subgraph titles are muted unless a palette tone recolours them, and
  // styled subgraphs get no tone.
  for (const subgraph of input.subgraphs) {
    check(subgraph.id, declarations(subgraph, subgraph.classes), palette.muted);
  }

  return [...inks].flatMap(([ink, ids], index) => [
    `classDef vcInk${index} color:${ink}`,
    `class ${ids.join(",")} vcInk${index}`,
  ]);
}

/**
 * Keys a diagram's own `init` directive or frontmatter may not set. HTML labels and theme CSS
 * are mounted while Mermaid lays the diagram out, before sanitizing, so a diagram must not turn
 * them on; the theme is ours too. A diagram may still opt into `look: handDrawn`.
 */
export const MERMAID_SECURE_KEYS = [
  'secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'suppressErrorRendering', 'maxEdges',
  'htmlLabels', 'themeCSS', 'theme', 'themeVariables', 'fontFamily', 'darkMode',
] as const;

/** The whole `mermaid.initialize` config for one palette: strict security, SVG text labels only. */
export function mermaidConfig(palette: DiagramPalette): Record<string, unknown> {
  const { themeVariables, themeCSS } = buildMermaidTheme(palette);
  return {
    startOnLoad: false,
    securityLevel: 'strict',
    suppressErrorRendering: true,
    secure: [...MERMAID_SECURE_KEYS],
    maxTextSize: 64 * 1024,
    htmlLabels: false,
    theme: 'base',
    look: 'classic',
    handDrawnSeed: 1,
    darkMode: palette.dark,
    fontFamily: palette.fontFamily,
    themeVariables,
    themeCSS,
    flowchart: { htmlLabels: false, curve: 'basis', padding: 16, nodeSpacing: 44, rankSpacing: 52, diagramPadding: 16, wrappingWidth: 220 },
    sequence: { mirrorActors: false, actorMargin: 56, boxMargin: 12, noteMargin: 12 },
    // Charts: no fixed pixel width, so they scale with the column like every other diagram.
    xyChart: { width: 640, height: 360 },
    gantt: { useWidth: 720, barHeight: 22, barGap: 6, topPadding: 44, fontSize: 12, sectionFontSize: 12 },
    pie: { textPosition: 0.72 },
  };
}
