/** True on Apple platforms, where the command key (⌘) is the shortcut modifier. `false` during server rendering. */
export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const platform = nav.userAgentData?.platform || nav.platform || nav.userAgent || "";
  return /mac|iphone|ipad|ipod/i.test(platform);
}

/** The shortcut modifier to show in hints: "⌘" on Mac, "Ctrl" elsewhere. */
export function modKey(): "⌘" | "Ctrl" {
  return isMac() ? "⌘" : "Ctrl";
}
