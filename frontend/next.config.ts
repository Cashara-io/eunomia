import type { NextConfig } from "next";

const backend = process.env.BACKEND_INTERNAL_URL || "http://localhost:8001";

const nextConfig: NextConfig = {
  async headers() {
    return [
      {
        source: "/:path*",
        headers: [
          // anti-clickjacking only: a full CSP would block the inline theme script and three.js
          { key: "Content-Security-Policy", value: "frame-ancestors 'none'" },
          { key: "X-Frame-Options", value: "DENY" },
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
        ],
      },
    ];
  },
  async rewrites() {
    return [
      { source: "/api/:path*", destination: `${backend}/api/:path*` },
      { source: "/mcp", destination: `${backend}/mcp` },
      // an extension's webhook inbox (the URL shown on the Connectors page)
      { source: "/hooks/:path*", destination: `${backend}/hooks/:path*` },
      // health probes through the one public port
      { source: "/healthz", destination: `${backend}/healthz` },
      { source: "/readyz", destination: `${backend}/readyz` },
      // OAuth for MCP clients: discovery documents and the protocol endpoints (the consent page itself is /consent)
      { source: "/.well-known/:path*", destination: `${backend}/.well-known/:path*` },
      { source: "/oauth/:path*", destination: `${backend}/oauth/:path*` },
    ];
  },
};

export default nextConfig;
