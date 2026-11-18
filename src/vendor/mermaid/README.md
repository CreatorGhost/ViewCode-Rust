# Mermaid (vendored)

`mermaid.min.js` draws ```mermaid diagrams in replies (`src/mermaid-render.ts`). It is loaded
only when the first diagram renders, as its own chunk; nothing imports it at startup.

- Source: `mermaid@11.17.2` on npm, file `dist/mermaid.min.js` (the same version ViewCode pins)
- Tarball: https://registry.npmjs.org/mermaid/-/mermaid-11.17.2.tgz
- Tarball integrity: `sha512-V6K3C8EBdEsPFZXSKMJe6ppQOENxuHARr9GvHX4hh47lAbhMRD9qf4oEK7LoaRQxULMa80/qt5gHO73aCleBBg==`
- Upstream file SHA-256: `581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8`
- Change: the last line, `globalThis["mermaid"] = globalThis.__esbuild_esm_mermaid_nm["mermaid"].default;`,
  is replaced with `export default __esbuild_esm_mermaid_nm["mermaid"].default;` so Vite can import
  it as a module (and nothing is put on `globalThis`). Nothing else is modified.
- License: MIT, see `LICENSE`. The build bundles third-party libraries (d3, dagre, DOMPurify,
  KaTeX, cytoscape, lodash and others); their licence banners are kept in the file.

It is vendored rather than installed so the shared `node_modules` stays untouched and the
exact build is reviewable. The single-file build is used (not the ESM build, which is ~100
chunks) so the whole renderer is one lazily loaded file.

To update: `npm pack mermaid@<version>`, copy `package/dist/mermaid.min.js` and `package/LICENSE`,
reapply the export change, check `tests/diagrams.test.mjs` still passes (it checks the change,
that the file has no `eval`/`new Function`, and that nothing imports it eagerly), and update the
version, integrity and hash above and in `THIRD_PARTY_NOTICES`.
