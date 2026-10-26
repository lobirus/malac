// What an approval request shows: a title and, behind "Show details", the
// full content. Enabling a plugin (`malac_configure` with `{plugin,
// contents}`) lists what it would run: hooks, MCP servers, scripts.

export interface ApprovalText {
  title: string;
  details: string | null;
  /** A plugin that runs code: worth a closer look before allowing. */
  plugin: string | null;
}

export function describeApproval(tool: string, summary: string, input: unknown): ApprovalText {
  const fields = (input && typeof input === "object" ? input : {}) as Record<string, unknown>;
  if (tool === "malac_configure" && typeof fields.plugin === "string") {
    const contents = typeof fields.contents === "string" ? fields.contents.trim() : "";
    return {
      title: summary ? capitalize(summary) : `Enable plugin ${fields.plugin}`,
      details: contents || null,
      plugin: fields.plugin,
    };
  }
  const title = summary ? `${tool}: ${summary}` : tool;
  if (tool === "bash" && typeof fields.command === "string") {
    const command = fields.command.trim();
    return { title, details: command.includes("\n") || command.length > 120 ? command : null, plugin: null };
  }
  const json = JSON.stringify(input ?? {}, null, 2);
  return { title, details: json && json !== "{}" && json !== "null" ? json : null, plugin: null };
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
