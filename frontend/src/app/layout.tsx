import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";
import "blobatar/motion.css";
import Providers from "@/components/Providers";

const geist = Geist({
  variable: "--font-geist",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Eunomia",
  description: "Shared memory for the AI apps you already use",
};

const DIRECTION_CONTRACT = `
THESIS: Eunomia is shared memory for the AI apps people already use. Make the next step obvious for people who should not need to understand MCP.
OWN-WORLD: Eunomia Bauhaus app design (spec v1.0, 05 Nov 2026). Warm paper #FAFAF7, ink #171717, 232px ink sidebar, blue #3159B6 primary actions and selected nav, red only for destructive or failing, yellow only as a fill. Geist. Lucide icons. 4px corners. Tokens prefixed --bauhaus-app-.
STORY: Choose your AI app, follow its own steps, sign in and approve, then try a recall. Then bring in knowledge, find it, and control who can see it.
FIRST VIEWPORT: Ink sidebar left; Home asks "Which AI app do you use?" with ChatGPT, Claude Code, Codex and Other apps, nothing preselected.
FINISH: status in words, confirmations name target and consequence, success only from real backend state. DESIGN.md is the source of truth.
`;

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${geist.variable} ${geistMono.variable} h-full antialiased`}>
      <body className="min-h-full flex">
        <div hidden dangerouslySetInnerHTML={{ __html: `<!--${DIRECTION_CONTRACT}-->` }} />
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
