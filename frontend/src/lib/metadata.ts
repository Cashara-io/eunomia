type PublicUrlEnv = { FRONTEND_URL?: string; PUBLIC_URL?: string };

export function configuredMetadataBase(
  env: PublicUrlEnv = {
    FRONTEND_URL: process.env.FRONTEND_URL,
    PUBLIC_URL: process.env.PUBLIC_URL,
  },
): URL | null {
  for (const value of [env.FRONTEND_URL, env.PUBLIC_URL]) {
    if (!value?.trim()) continue;
    try {
      const url = new URL(value.trim());
      if (
        ["http:", "https:"].includes(url.protocol) &&
        !url.username &&
        !url.password
      )
        return new URL(url.origin);
    } catch {}
  }
  return null;
}

// With no public URL, private installs share the hostname that actually served the page.
export function requestMetadataBase(
  host: string | null,
  protocol: string | null,
): URL {
  if (host && !/[\s/\\?#@]/.test(host)) {
    try {
      return new URL(`${protocol === "https" ? "https" : "http"}://${host}`);
    } catch {}
  }
  return new URL("http://localhost:3000");
}
