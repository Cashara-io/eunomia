/** The two documented refusals when a model may not be called for an organisation (docs/errors.md), in words for
 * the people who hit them. `ai.allowance_used`: the month's allowance on the server's key is used up (an owner can
 * add the organisation's own provider key; it renews at the start of next month, UTC). `ai.disabled`: an owner or
 * admin switched AI off for the organisation. */
export const AI_ALLOWANCE_USED = "ai.allowance_used";
export const AI_DISABLED = "ai.disabled";

export type AiRefusal = { code: string; title: string; body: string };

export function aiRefusal(code: string | undefined | null, canManage: boolean): AiRefusal | null {
  if (code === AI_ALLOWANCE_USED) {
    return {
      code,
      title: "This workspace has used its AI allowance for this month.",
      body: canManage
        ? "Add your organisation's own provider key to keep going now, or wait: the allowance renews at the start of next month (UTC). Saving, reading and keyword search still work."
        : "Ask an owner of this workspace to add the organisation's own provider key, or wait: the allowance renews at the start of next month (UTC). Saving, reading and keyword search still work.",
    };
  }
  if (code === AI_DISABLED) {
    return {
      code,
      title: "AI is switched off for this workspace.",
      body: canManage
        ? "You can switch it back on in Settings > Organisation. Saving, reading and keyword search still work."
        : "An owner or admin of this workspace can switch it back on. Saving, reading and keyword search still work.",
    };
  }
  return null;
}
