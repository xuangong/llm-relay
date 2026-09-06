export type ClaudeModelRole = "main" | "subagent" | "haiku";

function modelFamilyId(id: string): string {
  const slash = id.lastIndexOf("/");
  return id.slice(slash + 1).toLowerCase();
}

export function isClaudeFamilyModel(id: string): boolean {
  return modelFamilyId(id).startsWith("claude");
}

export function isGpt56Model(id: string): boolean {
  return modelFamilyId(id).startsWith("gpt-5.6");
}

export function stripMainContextSuffix(id: string): string {
  return id.replace(/\[1m\]$/i, "");
}

export function withMainContextSuffix(id: string): string {
  const base = stripMainContextSuffix(id);
  return base ? `${base}[1m]` : "";
}

export function claudeCodeModels(ids: string[]): string[] {
  return ids.filter((id) => isClaudeFamilyModel(id) || isGpt56Model(id));
}

export function allClaudeRolesUseClaudeFamily(
  main: string,
  subagent: string,
  haiku: string,
): boolean {
  return [main, subagent, haiku].every((id) =>
    isClaudeFamilyModel(stripMainContextSuffix(id)),
  );
}

export function claudeRoleModels(
  ids: string[],
  role: ClaudeModelRole,
): string[] {
  const candidates = claudeCodeModels(ids);
  return role === "main" ? candidates.map(withMainContextSuffix) : candidates;
}

export function codexModels(ids: string[]): string[] {
  return ids.filter((id) => {
    const lower = id.toLowerCase();
    return /gpt-[5-9]/.test(lower) || /\bo[1-9]/.test(lower);
  });
}

export function geminiModels(ids: string[]): string[] {
  return ids.filter((id) => id.toLowerCase().includes("gemini"));
}

export function reconcileModelSelection(
  current: string,
  candidates: string[],
): string {
  return candidates.includes(current) ? current : "";
}
