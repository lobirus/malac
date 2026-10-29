// Models grouped by provider (the part of `provider/model` before the first
// slash), so model pickers show each gateway under its own heading and
// switching between them is one glance: the chat's model menu (through
// ChatState.modelGroups) and the voice settings' QuickPicks. Free of the
// vscode module.

export interface ModelGroup {
  provider: string;
  models: { spec: string; name: string }[];
}

/** `openrouter/anthropic/claude` → ["openrouter", "anthropic/claude"]; no slash → ["", spec]. */
export function splitModel(spec: string): [string, string] {
  const index = spec.indexOf("/");
  return index < 0 ? ["", spec] : [spec.slice(0, index), spec.slice(index + 1)];
}

/** Groups in the order providers first appear; each model once. */
export function groupModels(specs: readonly string[]): ModelGroup[] {
  const groups = new Map<string, ModelGroup>();
  const seen = new Set<string>();
  for (const spec of specs) {
    if (!spec || seen.has(spec)) continue;
    seen.add(spec);
    const [provider, name] = splitModel(spec);
    let group = groups.get(provider);
    if (!group) groups.set(provider, (group = { provider, models: [] }));
    group.models.push({ spec, name: name || spec });
  }
  return [...groups.values()];
}
