// The version this frontend was built as, shown the same way everywhere (sidebar footer, Settings >
// Updates): "2.0.9" for a release, "dev" for a local build. Never "vdev", never "unknown".
const raw = process.env.NEXT_PUBLIC_APP_VERSION ?? "dev";
export const APP_VERSION = raw.replace(/^v/, "");
/** "v2.0.9" for a release; "dev" for a local build. */
export const versionLabel = (v: string = APP_VERSION) => (/^\d/.test(v) ? `v${v.replace(/^v/, "")}` : v);
/** The words for an updater that isn't reporting, used by the sidebar and Settings alike. */
export const UPDATER_OFF = "Updater not running";
