export type ClaudeModelRole = "main" | "subagent" | "haiku";

function modelFamilyId(id: string): string {
  const slash = id.lastIndexOf("/");
  return id.slice(slash + 1).toLowerCase();
}

export function isClaudeFamilyModel(id: string): boolean {
  return modelFamilyId(id).startsWith("claude");
}

export function stripMainContextSuffix(id: string): string {
  return id.replace(/\[1m\]$/i, "");
}

export function withMainContextSuffix(id: string): string {
  const base = stripMainContextSuffix(id);
  return base ? `${base}[1m]` : "";
}

export function claudeCodeModels(ids: string[]): string[] {
  return [...new Set(ids.filter(Boolean))];
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
  return role === "main"
    ? [...new Set(candidates.map(withMainContextSuffix))]
    : candidates;
}

export function codexModels(ids: string[]): string[] {
  return [...new Set(ids.filter(Boolean))];
}

export function geminiModels(ids: string[]): string[] {
  return [
    ...new Set(ids.filter((id) => modelFamilyId(id).startsWith("gemini"))),
  ];
}

export function searchModels(ids: string[], query: string): string[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return ids;

  return ids.filter((id) => {
    const model = id.toLowerCase();
    return terms.every((term) => {
      // Match non-adjacent characters too, e.g. "gpt54" -> "gpt-5.4".
      let position = 0;
      for (const character of term) {
        const index = model.indexOf(character, position);
        if (index === -1) return false;
        position = index + 1;
      }
      return true;
    });
  });
}

export function reconcileModelSelection(
  current: string,
  candidates: string[],
): string {
  return candidates.includes(current) ? current : "";
}
