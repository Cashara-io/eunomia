"use client";

import { useSyncExternalStore } from "react";
import type { WebHaptics } from "web-haptics";

const KEY = "eunomia-haptics";
const EVENT = "eunomia-haptics-change";
let fallback = true;
let engine: WebHaptics | undefined;
let pending: Promise<WebHaptics> | undefined;

function enabled() {
  try {
    return localStorage.getItem(KEY) !== "off";
  } catch {
    return fallback;
  }
}
function subscribe(notify: () => void) {
  window.addEventListener("storage", notify);
  window.addEventListener(EVENT, notify);
  return () => {
    window.removeEventListener("storage", notify);
    window.removeEventListener(EVENT, notify);
  };
}
export function setHapticsEnabled(on: boolean) {
  fallback = on;
  try {
    localStorage.setItem(KEY, on ? "on" : "off");
  } catch {}
  if (!on) engine?.cancel();
  window.dispatchEvent(new Event(EVENT));
}
export function useHapticsEnabled() {
  return useSyncExternalStore(subscribe, enabled, () => true);
}
function allowed() {
  return (
    typeof window !== "undefined" &&
    enabled() &&
    window.matchMedia("(pointer: coarse)").matches &&
    !window.matchMedia("(prefers-reduced-motion: reduce)").matches &&
    typeof navigator.vibrate === "function"
  );
}

// Loaded only for an intentional action on a supported touch device. Never plays debug audio.
export function haptic(kind: "success" | "error" | "send") {
  if (!allowed()) return;
  pending ??= import("web-haptics").then(
    ({ WebHaptics }) =>
      (engine ??= new WebHaptics({ debug: false, showSwitch: false })),
  );
  void pending
    .then((instance) => {
      if (allowed())
        return instance.trigger(kind === "send" ? 15 : kind, {
          intensity: 0.35,
        });
    })
    .catch(() => {
      pending = undefined;
    });
}
