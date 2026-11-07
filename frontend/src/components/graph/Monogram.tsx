// A person's initials in a coloured circle: the avatar for person nodes in the
// graph (SVG) and in the inspector (HTML).

/** Up to two letters: first and last word ("Priya Nair" -> "PN"), or the first letter of a single word. */
export function initials(name: string): string {
  const words = name
    .trim()
    .split(/[\s._@-]+/)
    .filter((w) => /\p{L}|\p{N}/u.test(w));
  if (words.length === 0) return "?";
  const first = (w: string) => [...w.replace(/^[^\p{L}\p{N}]+/u, "")][0] ?? "";
  const letters = words.length === 1 ? first(words[0]) : first(words[0]) + first(words[words.length - 1]);
  return letters.toUpperCase() || "?";
}

/** The monogram inside an SVG, centred on (0, 0). */
export function SvgMonogram({ name, r, color, ring, ringWidth }: { name: string; r: number; color: string; ring: string; ringWidth: number }) {
  return (
    <>
      <circle r={r} fill={color} stroke={ring} strokeWidth={ringWidth} />
      <text
        textAnchor="middle"
        dominantBaseline="central"
        fontSize={r * 0.78}
        fontWeight={600}
        fill="#fff"
        style={{ pointerEvents: "none" }}
        aria-hidden
      >
        {initials(name)}
      </text>
    </>
  );
}

/** The monogram as an HTML element, `size` px across. */
export default function Monogram({ name, size, color }: { name: string; size: number; color: string }) {
  return (
    <span
      aria-hidden
      className="inline-flex items-center justify-center rounded-full shrink-0 font-semibold select-none"
      style={{ width: size, height: size, background: color, color: "#fff", fontSize: Math.round(size * 0.4), borderRadius: 999 }}
    >
      {initials(name)}
    </span>
  );
}
