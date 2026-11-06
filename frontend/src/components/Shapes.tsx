/* Bauhaus shape compositions for empty space: auth, onboarding and empty
   states. Big flat shapes in the three vault colours, cropped by their
   panel's edge like the blog banners. They sit on --surface-raised, never
   behind text, never inside tables or forms. No outlines, shadows or
   gradients, and nothing loops: each composition settles in once (the
   surface's one moment). The auth and empty-state compositions are playable
   (ShapeField): drag and throw a shape and it springs back home. Under reduced
   motion they are still pictures. Decorative only, so every one is aria-hidden. */

import ShapeField, { type ShapeBody } from "@/components/ShapeField";

const P = "var(--vault-personal)";
const Y = "var(--vault-project)";
const B = "var(--vault-company)";
const INK = "var(--ink)";

/** Settle-in for a composition: each shape scales up from 0.96 and fades in,
 * staggered by --i. Under reduced motion they are simply there. */
const SETTLE = `
@keyframes shape-settle { from { opacity: 0.001; transform: scale(0.92); } }
.shape-art > * { transform-box: fill-box; transform-origin: center; animation: shape-settle 420ms var(--ease-out) both; animation-delay: calc(var(--i, 0) * 70ms); }
@media (prefers-reduced-motion: reduce) { .shape-art > * { animation: none; } }
`;

function i(n: number) {
  return { ["--i" as string]: n } as React.CSSProperties;
}

/** The auth panel's composition: fills the space under the promise, shapes
 * running off the bottom and right edges. Playable: drag and throw them. */
const AUTH_BODIES: ShapeBody[] = [
  { x: 580, y: 250, r: 150, draw: <circle r="150" fill={Y} /> },
  { x: 640, y: 480, r: 270, draw: <path d="M0 0H-270A270 270 0 0 1 0 -270Z" fill={B} /> },
  { x: 170, y: 430, r: 96, draw: <circle r="96" fill={P} /> },
  { x: 276, y: 195.33, r: 40, draw: <path d="M0 -45.33 38 22.67H-38Z" fill={INK} /> },
  { x: 406, y: 338, r: 26, a: 14, draw: <rect x="-22" y="-22" width="44" height="44" fill={P} /> },
  { x: 86, y: 262, r: 14, draw: <circle r="14" fill={B} /> },
];

export function AuthArt({ className = "" }: { className?: string }) {
  return <ShapeField viewBox="0 0 640 480" preserveAspectRatio="xMaxYMax slice" bodies={AUTH_BODIES} className={className} overflow />;
}

/** A thin band of the same composition for narrow screens, where the auth
 * panel collapses to the wordmark. */
export function AuthBand({ className = "" }: { className?: string }) {
  return (
    <svg viewBox="0 0 400 72" preserveAspectRatio="xMaxYMax slice" className={className} aria-hidden>
      <path d="M400 72H318A82 82 0 0 1 400 -10Z" fill={B} />
      <rect x="262" y="26" width="34" height="34" fill={Y} />
      <circle cx="228" cy="56" r="16" fill={P} />
      <path d="M180 30 194 54H166Z" fill={INK} />
    </svg>
  );
}

export type StepShape = "trio" | "circle" | "triangle" | "square";

/** Onboarding: each step has a shape of its own; the welcome step shows all
 * three together. A raised band at the top of the step, the shape cropped by
 * its right edge. `still` skips the settle-in where the step already has its
 * moment (the welcome heading). */
export function StepArt({ shape, still = false, className = "" }: { shape: StepShape; still?: boolean; className?: string }) {
  return (
    <div className={`relative overflow-hidden rounded-[10px] h-[104px] ${className}`} style={{ background: "var(--surface-raised)" }} aria-hidden>
      <svg viewBox="0 0 600 104" preserveAspectRatio="xMaxYMid slice" className="absolute inset-0 w-full h-full">
        {!still && <style>{SETTLE}</style>}
        <g className={still ? undefined : "shape-art"}>
          {shape === "trio" && (
            <>
              <circle cx="372" cy="70" r="54" fill={P} style={i(0)} />
              <rect x="444" y="16" width="88" height="88" fill={Y} style={i(1)} />
              <path d="M590 -6 660 118H520Z" fill={B} style={i(2)} />
            </>
          )}
          {shape === "circle" && (
            <>
              <circle cx="540" cy="92" r="96" fill={P} style={i(0)} />
              <g style={i(1)}><rect x="412" y="34" width="22" height="22" fill={Y} transform="rotate(12 423 45)" /></g>
            </>
          )}
          {shape === "triangle" && (
            <>
              <path d="M520 -10 640 120H400Z" fill={B} style={i(0)} />
              <circle cx="372" cy="70" r="12" fill={P} style={i(1)} />
            </>
          )}
          {shape === "square" && (
            <>
              <g style={i(0)}><rect x="470" y="14" width="140" height="140" fill={Y} transform="rotate(-8 540 84)" /></g>
              <path d="M410 30 430 66H390Z" fill={INK} style={i(1)} />
            </>
          )}
        </g>
      </svg>
    </div>
  );
}

/** A step's own small shape, for the onboarding progress labels. */
export function StepDot({ shape, dim = false }: { shape: Exclude<StepShape, "trio">; dim?: boolean }) {
  const fill = dim ? "var(--border-strong)" : shape === "circle" ? P : shape === "square" ? Y : B;
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden className="shrink-0">
      {shape === "circle" && <circle cx="5" cy="5" r="5" fill={fill} />}
      {shape === "square" && <rect width="10" height="10" fill={fill} />}
      {shape === "triangle" && <path d="M5 0 10 10H0Z" fill={fill} />}
    </svg>
  );
}

export type EmptyVariant = "documents" | "people" | "chat" | "memory" | "sources" | "spaces";

const EMPTY_BODIES: Record<EmptyVariant, ShapeBody[]> = {
  documents: [
    { x: 214, y: 72, r: 42, draw: <rect x="-36" y="-46" width="72" height="92" rx="4" fill={Y} /> },
    { x: 266, y: 92, r: 30, draw: <path d="M-40 0a40 40 0 0 0 80 0Z" fill={P} /> },
    { x: 150, y: 110.67, r: 17, draw: <path d="M0 -18.67 16 9.33H-16Z" fill={B} /> },
    { x: 56, y: 44, r: 10, draw: <circle r="10" fill={P} /> },
  ],
  people: [
    { x: 196, y: 52, r: 26, draw: <circle r="26" fill={P} /> },
    { x: 196, y: 140, r: 30, draw: <path d="M-38 0a38 38 0 0 1 76 0Z" fill={Y} /> },
    { x: 280, y: 106.67, r: 55, draw: <path d="M0 -66.67 54 33.33H-54Z" fill={B} /> },
    { x: 58, y: 94, r: 10, draw: <rect x="-8" y="-8" width="16" height="16" fill={Y} /> },
  ],
  chat: [
    { x: 300, y: 22, r: 64, draw: <circle r="64" fill={Y} /> },
    {
      x: 190,
      y: 76,
      r: 34,
      draw: (
        <>
          <circle r="32" fill={B} />
          <path d="M-22 24 6 28-18 50Z" fill={B} />
        </>
      ),
    },
    { x: 247, y: 103, r: 13, draw: <rect x="-11" y="-11" width="22" height="22" fill={P} /> },
    { x: 64, y: 56, r: 14, draw: <path d="M0 -16 14 8H-14Z" fill={INK} /> },
  ],
  memory: [
    { x: 170, y: 134, r: 52, draw: <circle r="52" fill={P} /> },
    { x: 244, y: 62, r: 26, draw: <rect x="-22" y="-22" width="44" height="44" fill={Y} /> },
    { x: 300, y: 62, r: 40, draw: <path d="M0 -48 40 24H-40Z" fill={B} /> },
    { x: 58, y: 56, r: 9, draw: <circle r="9" fill={B} /> },
  ],
  spaces: [
    { x: 195, y: 99, r: 30, draw: <rect x="-45" y="-19" width="90" height="38" rx="4" fill={B} /> },
    { x: 211, y: 67, r: 30, draw: <rect x="-45" y="-19" width="90" height="38" rx="4" fill={Y} /> },
    { x: 290, y: 34, r: 30, draw: <circle r="30" fill={P} /> },
    { x: 58, y: 86, r: 14, draw: <path d="M0 -16 14 8H-14Z" fill={INK} /> },
  ],
  sources: [
    { x: 261, y: 69, r: 38, draw: <rect x="-35" y="-35" width="70" height="70" fill={Y} /> },
    { x: 171.33, y: 70, r: 30, draw: <path d="M-21.33 -24 42.67 0-21.33 24Z" fill={B} /> },
    { x: 300, y: 134, r: 26, draw: <circle r="26" fill={P} /> },
    { x: 60, y: 90, r: 9, draw: <circle r="9" fill={P} /> },
  ],
};

/** Empty states: one composition of two or three shapes in a raised panel
 * beside (or above, on phones) the sentence and the one action. Drawn on a
 * wide 320 x 140 canvas anchored right: a 200 x 140 box shows the main group,
 * a wide phone box also shows the small accent on the left. Playable: drag and
 * throw the shapes; they spring back. */
export function EmptyArt({ variant, className = "" }: { variant: EmptyVariant; className?: string }) {
  return (
    <div className={`relative overflow-hidden rounded-[10px] shrink-0 ${className}`} style={{ background: "var(--surface-raised)" }} aria-hidden>
      <ShapeField viewBox="0 0 320 140" preserveAspectRatio="xMaxYMid slice" bodies={EMPTY_BODIES[variant]} className="absolute inset-0 w-full h-full" />
    </div>
  );
}

/** The whole empty state: art, one sentence, one action. `bare` drops the
 * ledger frame for use inside a container that already has one. */
export function EmptyState({
  variant,
  title,
  children,
  action,
  bare = false,
  className = "",
}: {
  variant: EmptyVariant;
  title: React.ReactNode;
  children?: React.ReactNode;
  action?: React.ReactNode;
  bare?: boolean;
  className?: string;
}) {
  return (
    <section className={`${bare ? "" : "ledger"} p-3 flex flex-col sm:flex-row gap-4 sm:gap-6 sm:items-center ${className}`}>
      <EmptyArt variant={variant} className="w-full h-[120px] sm:w-[200px] sm:h-[140px]" />
      <div className="flex flex-col gap-2 items-start px-2 pb-2 sm:p-0 sm:pr-4">
        <h2 className="section-title">{title}</h2>
        {children && (
          <p className="text-[14px] leading-relaxed max-w-[52ch]" style={{ color: "var(--ink-dim)" }}>
            {children}
          </p>
        )}
        {action && <div className="pt-1">{action}</div>}
      </div>
    </section>
  );
}
