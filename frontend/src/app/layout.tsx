import type { Metadata } from "next";
// Self-hosted (the geist package wraps next/font/local): builds and pages never reach out to Google,
// so firewalled and air-gapped installs get the real typeface (issue #155).
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "./globals.css";
import "blobatar/motion.css";
import Providers from "@/components/Providers";

const geist = GeistSans;
const geistMono = GeistMono;

export const metadata: Metadata = {
  title: "Eunomia",
  description: "Shared memory for the AI apps you already use",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${geist.variable} ${geistMono.variable} h-full antialiased`}>
      <body className="min-h-full flex">
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
