"use client";

import { lazy, Suspense, useSyncExternalStore } from "react";
import { formatNumber } from "@/lib/numbers";

const TextMorph = lazy(() =>
  import("torph/react").then((m) => ({ default: m.TextMorph })),
);
const subscribe = (notify: () => void) => {
  const query = window.matchMedia("(prefers-reduced-motion: reduce)");
  query.addEventListener("change", notify);
  return () => query.removeEventListener("change", notify);
};
const reduced = () =>
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export default function MorphText({
  children,
  className,
}: {
  children: string;
  className?: string;
}) {
  const motionReduced = useSyncExternalStore(subscribe, reduced, () => true);
  const still = motionReduced || children.length > 48;
  return (
    <span className={className} data-text-morph={still ? "off" : "on"}>
      <span className="sr-only">{children}</span>
      <span aria-hidden="true">
        {still ? (
          children
        ) : (
          <Suspense fallback={children}>
            <TextMorph duration={220} respectReducedMotion>
              {children}
            </TextMorph>
          </Suspense>
        )}
      </span>
    </span>
  );
}

export function MorphCount({ value }: { value: number }) {
  return (
    <MorphText className="font-mono">{formatNumber(value, null)}</MorphText>
  );
}
