// Whether the live update stream (`/api/events/ws`, see live.ts) is connected. Kept apart from live.ts
// so the query hooks can read it without importing each other.
import { LIVE_POLL_MS } from "./client";

export type LiveStatus = "connecting" | "live" | "offline";

let status: LiveStatus = "connecting";
const listeners = new Set<() => void>();

export const getLiveStatus = () => status;

export function setLiveStatus(next: LiveStatus) {
  if (next === status) return;
  status = next;
  listeners.forEach((l) => l());
}

export function subscribeLiveStatus(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/**
 * `refetchInterval` for views an agent can change: no polling while the live stream is connected (it
 * says what changed), polling every LIVE_POLL_MS while it is not. Hidden tabs never poll
 * (refetchIntervalInBackground defaults to false).
 */
export const livePoll = () => (status === "live" ? false : LIVE_POLL_MS);
