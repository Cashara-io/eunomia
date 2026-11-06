"use client";

import { DownloadSimpleIcon } from "@phosphor-icons/react/ssr";
import { API_URL } from "@/lib/api";

/** Plays (or downloads) the audio Eunomia kept for one Pocket recording (#117). Render it only when the recording has audio. */
export default function PocketAudioPlayer({ recordingId, title }: { recordingId: string; title?: string }) {
  const src = `${API_URL}/api/connectors/pocketai/recordings/${encodeURIComponent(recordingId)}/audio`;
  return (
    <div className="flex items-center gap-3 flex-wrap">
      <span className="label w-32 shrink-0">Audio</span>
      {/* the session cookie authorizes it, as for every other /api request */}
      <audio controls preload="none" src={src} className="max-w-full h-9" aria-label={title ? `Audio of ${title}` : "Recording audio"} />
      <a className="btn btn-sm" href={`${src}?download=true`} download>
        <DownloadSimpleIcon weight="duotone" size={13} aria-hidden />
        Download
      </a>
    </div>
  );
}
