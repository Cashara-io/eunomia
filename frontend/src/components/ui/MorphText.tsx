"use client";

import { useSyncExternalStore } from "react";
import { formatNumber } from "@/lib/numbers";
import styles from "./MorphText.module.css";

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
    <span
      key={children}
      className={`${className ?? ""} ${still ? "" : styles.enter}`}
      data-text-morph={still ? "off" : "on"}
    >
      {children}
    </span>
  );
}

export function MorphCount({ value }: { value: number }) {
  return (
    <MorphText className="font-mono">{formatNumber(value, null)}</MorphText>
  );
}
