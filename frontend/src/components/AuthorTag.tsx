/** Figma-style multiplayer name tag: who wrote a memory, record or edit.
 * Pass the real author (an owner email, an agent/client name, or a source
 * key). Never invent one. Colour is stable per author name. */

const AUTHOR_COLORS = 6;

// Well-known agents get fixed, distinct colours so two of them never share one.
const KNOWN: Record<string, number> = {
  "claude code": 1,
  claude: 1,
  codex: 6,
  cursor: 3,
  eunomia: 5,
  gemini: 2,
  windsurf: 4,
};

function authorSlot(name: string): number {
  const known = KNOWN[name.trim().toLowerCase()];
  if (known) return known;
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return (h % AUTHOR_COLORS) + 1;
}

export function authorColor(name: string): string {
  return `var(--author-${authorSlot(name)})`;
}

export function authorLabel(name: string): string {
  return name.includes("@") ? name.split("@")[0] : name;
}

export function CursorGlyph({ color, size = 14 }: { color: string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden style={{ flexShrink: 0 }}>
      <path d="M2 1.5 13.5 7 8.3 8.4 6 13.8Z" fill={color} stroke="var(--canvas)" strokeWidth="1" strokeLinejoin="round" />
    </svg>
  );
}

export default function AuthorTag({ name, cursor = false, title }: { name: string; cursor?: boolean; title?: string }) {
  const slot = authorSlot(name);
  const color = `var(--author-${slot})`;
  const tag = (
    <span className="author-tag" style={{ ["--author" as string]: color, ["--author-on" as string]: `var(--on-author-${slot})` }} title={title ?? name}>
      <span className="truncate">{authorLabel(name)}</span>
    </span>
  );
  if (!cursor) return tag;
  // Cursor pinned to the tag's top-left corner, like a Figma name tag.
  return (
    <span className="relative inline-flex">
      <span className="absolute -left-2 -top-3 pointer-events-none">
        <CursorGlyph color={color} size={16} />
      </span>
      {tag}
    </span>
  );
}
