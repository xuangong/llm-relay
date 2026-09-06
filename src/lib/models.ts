export type ClaudeModelRole = "main" | "subagent" | "haiku";

const CLAUDE_ROLE_FALLBACK: Record<ClaudeModelRole, string> = {
  main: "sol",
  subagent: "terra",
  haiku: "luna",
};

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

export function preferredClaudeCodeModel(
  ids: string[],
  role: ClaudeModelRole,
): string {
  const claudeCandidates = ids.filter(isClaudeFamilyModel);
  const preferredFamily =
    role === "main" ? "opus" : role === "subagent" ? "sonnet" : "haiku";
  const selected =
    claudeCandidates.find((id) =>
      modelFamilyId(id).includes(preferredFamily),
    ) ??
    (claudeCandidates.length > 0
      ? claudeCandidates[0]
      : claudeCodeModels(ids).find((id) =>
          id.toLowerCase().includes(CLAUDE_ROLE_FALLBACK[role]),
        )) ??
    "";
  return role === "main" ? withMainContextSuffix(selected) : selected;
}

export function codexModels(ids: string[]): string[] {
  return ids.filter((id) => {
    const lower = id.toLowerCase();
    return /gpt-[5-9]/.test(lower) || /\bo[1-9]/.test(lower);
  });
}

export function preferredCodexModel(ids: string[]): string {
  return (
    codexModels(ids).find((id) => /gpt-[5-9]/i.test(id)) ??
    codexModels(ids).find((id) => /\bo[1-9]/i.test(id)) ??
    ""
  );
}

export function preferredCodexSubagentModel(
  ids: string[],
  mainModel?: string,
): string {
  const candidates = codexModels(ids);
  return (
    candidates.find((id) => {
      const family = modelFamilyId(id);
      return family.startsWith("gpt-") && family.endsWith("-fast");
    }) ??
    candidates.find((id) => id === mainModel) ??
    preferredCodexModel(ids)
  );
}

export function geminiModels(ids: string[]): string[] {
  return ids.filter((id) => id.toLowerCase().includes("gemini"));
}

export function preferredGeminiModel(ids: string[]): string {
  return geminiModels(ids)[0] ?? "";
}

export function reconcileModelSelection(
  current: string,
  candidates: string[],
  fallback: string,
): string {
  return candidates.includes(current) ? current : fallback;
}
