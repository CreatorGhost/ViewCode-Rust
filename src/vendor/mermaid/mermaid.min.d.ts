/** The part of Mermaid's API PandaCode uses (mermaid@11.17.2, dist/mermaid.min.js). */
export interface Mermaid {
  initialize(config: Record<string, unknown>): void;
  render(id: string, text: string): Promise<{ svg: string }>;
  parse(text: string, options?: { suppressErrors?: boolean }): Promise<unknown>;
  mermaidAPI: { getDiagramFromText(text: string): Promise<{ type: string; db: unknown }> };
}
declare const mermaid: Mermaid;
export default mermaid;
