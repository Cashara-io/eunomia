import type { Metadata, Viewport } from "next";
import { headers } from "next/headers";
import { configuredMetadataBase, requestMetadataBase } from "@/lib/metadata";
// Self-hosted (the geist package wraps next/font/local): builds and pages never reach out to Google,
// so firewalled and air-gapped installs get the real typeface (issue #155).
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "./globals.css";
import "blobatar/motion.css";
import Providers from "@/components/Providers";

const geist = GeistSans;
const geistMono = GeistMono;

export async function generateMetadata(): Promise<Metadata> {
  const configured = configuredMetadataBase();
  const request = configured ? null : await headers();
  return {
    metadataBase:
      configured ??
      requestMetadataBase(
        request?.get("host") ?? null,
        request?.get("x-forwarded-proto") ?? null,
      ),
    title: "Eunomia",
    openGraph: {
      type: "website",
      locale: "en_AU",
      siteName: "Eunomia",
      title: "Eunomia",
      description: "Shared memory for the AI apps you already use",
    },
    twitter: {
      card: "summary_large_image",
      title: "Eunomia",
      description: "Shared memory for the AI apps you already use",
    },
    description: "Shared memory for the AI apps you already use",
  };
}

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#FAFAF7" },
    { media: "(prefers-color-scheme: dark)", color: "#121211" },
  ],
};

// Dark is opt-in and saved per browser; set it before first paint so the page never flashes light.
const THEME_SCRIPT = `try{if(localStorage.getItem("eunomia-theme")==="dark")document.documentElement.dataset.theme="dark"}catch(e){}`;

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html
      lang="en"
      className={`${geist.variable} ${geistMono.variable} h-full antialiased`}
      suppressHydrationWarning
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body className="min-h-full flex">
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
