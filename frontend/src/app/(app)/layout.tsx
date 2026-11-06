import AuthGuard from "@/components/AuthGuard";
import CommandPalette from "@/components/CommandPalette";
import Sidebar from "@/components/Sidebar";
import LiveUpdates from "@/components/live/LiveUpdates";
import { InputModeTracker } from "@/components/bits/motion";

export default function AppLayout({ children }: { children: React.ReactNode }) {
  return (
    <AuthGuard>
      <InputModeTracker />
      <LiveUpdates />
      <div className="flex flex-col md:flex-row flex-1 min-w-0">
        <Sidebar />
        <main id="main" className="canvas-grid flex-1 min-w-0 px-4 py-6 sm:px-8 lg:px-12 lg:py-[38px]">{children}</main>
      </div>
      <CommandPalette />
    </AuthGuard>
  );
}
