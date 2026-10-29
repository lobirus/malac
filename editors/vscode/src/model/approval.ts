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

/**
 * One line for a waiting request outside the chat, saying which agent wants
 * what: "main in api wants to use bash: cargo publish".
 */
export function approvalHeadline(request: { agent_label: string; tool: string; summary: string; input: unknown }, tab: string | null | undefined): string {
  const where = tab ? ` in ${tab}` : "";
  const { title, plugin } = describeApproval(request.tool, request.summary, request.input);
  if (plugin) return `${request.agent_label}${where} asks to ${title.charAt(0).toLowerCase()}${title.slice(1)}. It runs code on the daemon's machine.`;
  return `${request.agent_label}${where} wants to use ${truncate(title, 200)}`;
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
