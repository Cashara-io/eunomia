"use client";
import { createContext, useContext, useState, type ReactNode } from "react";
const KindsContext = createContext<string[]>([]);
const SetKindsContext = createContext<(kinds: string[]) => void>(() => {});
export function OverviewHighlight({ children }: { children: ReactNode }) {
  const [kinds, setKinds] = useState<string[]>([]);
  return (
    <SetKindsContext value={setKinds}>
      <KindsContext value={kinds}>{children}</KindsContext>
    </SetKindsContext>
  );
}
export const useOverviewHighlight = () => useContext(KindsContext);
export const useSetOverviewHighlight = () => useContext(SetKindsContext);
