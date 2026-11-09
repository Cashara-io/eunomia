"use client";

import { useEffect, useRef, type ReactNode } from "react";
import type { ShaderController, SweepHandle } from "glimm";
import { useReducedMotion } from "@/components/bits/motion";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

// Browser power APIs are optional. Browsers without them still honour reduced motion.
declare global {
  interface Navigator {
    readonly connection?: { readonly saveData?: boolean };
    getBattery?: () => Promise<{ charging: boolean; level: number }>;
  }
}

// Resolving several historical blocks together must never allocate one context per block.
let owner: HTMLCanvasElement | null = null;

export default function GenerationBox({
  active,
  children,
  className = "",
  announce = true,
}: {
  active: boolean;
  children: ReactNode;
  className?: string;
  announce?: boolean;
}) {
  return (
    <>
      {announce && (
        <span role="status" aria-live="polite" className="sr-only">
          {active ? "Generating" : ""}
        </span>
      )}
      <div
        className={`generation-box ${className}`}
        aria-busy={active}
        data-generation-box
      >
        {children}
        {active && <Sweep />}
      </div>
    </>
  );
}

function Sweep() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();
  useEffect(() => {
    const canvas = canvasRef.current;
    const layer = layerRef.current;
    if (!canvas || !layer || reduced) return;
    let disposed = false;
    let controller: ShaderController | null = null;
    let gl: WebGLRenderingContext | null = null;
    let sweep: SweepHandle | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function release() {
      clearTimeout(timer);
      sweep?.cancel();
      controller?.destroy();
      controller = null;
      // glimm 0.4.1 deletes GPU resources but does not relinquish the browser context.
      gl?.getExtension("WEBGL_lose_context")?.loseContext();
      gl = null;
      if (owner === canvas) owner = null;
    }
    function fallback() {
      release();
      if (layer) layer.dataset.mode = "fallback";
    }
    const start = async () => {
      if (
        navigator.connection?.saveData ||
        navigator.hardwareConcurrency <= 2 ||
        window.matchMedia("(update: slow)").matches ||
        document.hidden
      )
        return;
      if (navigator.getBattery) {
        const battery = await navigator.getBattery().catch(() => null);
        if (battery && !battery.charging && battery.level <= 0.2) return;
      }
      const { createShader, playSweep } = await import("glimm");
      if (disposed || document.hidden || (owner && owner !== canvas)) return;
      owner = canvas;
      gl = canvas.getContext("webgl", {
        alpha: true,
        antialias: false,
        premultipliedAlpha: true,
        powerPreference: "low-power",
      });
      if (!gl) {
        fallback();
        return;
      }
      const ink = getComputedStyle(document.documentElement)
        .getPropertyValue("--ink")
        .trim();
      // A neutral ink band keeps the vault colours reserved for memory spaces.
      const rgb = /^#[0-9a-f]{6}$/i.test(ink)
        ? [1, 3, 5].map((at) => parseInt(ink.slice(at, at + 2), 16) / 255)
        : [0.09, 0.09, 0.09];
      controller = createShader({
        canvas,
        palette: {
          a: [rgb[0], rgb[1], rgb[2]],
          b: [0, 0, 0],
          c: [0, 0, 0],
          d: [0, 0, 0],
        },
        bandTight: 18,
        feather: "auto",
        swellAmount: 0,
        rippleAmount: 0,
      });
      if (!controller) {
        fallback();
        return;
      }
      layer.dataset.mode = "webgl";
      const next = async () => {
        if (disposed || !controller) return;
        sweep = playSweep(controller, {
          sweepMs: 1800,
          outroMs: 180,
          peakAlpha: 0.24,
          easing: "linear",
          swellAmount: 0,
          rippleAmount: 0,
        });
        await sweep.done;
        if (!disposed && controller)
          timer = setTimeout(() => {
            void next().catch(fallback);
          }, 600);
      };
      await next();
    };
    const onLost = () => {
      if (!disposed) fallback();
    };
    const onVisibility = () => {
      if (document.hidden) fallback();
    };
    canvas.addEventListener("webglcontextlost", onLost);
    document.addEventListener("visibilitychange", onVisibility);
    void start().catch(fallback);
    return () => {
      disposed = true;
      canvas.removeEventListener("webglcontextlost", onLost);
      document.removeEventListener("visibilitychange", onVisibility);
      release();
    };
  }, [reduced]);
  return (
    <div
      ref={layerRef}
      className="generation-effect"
      data-generation-effect
      data-mode={reduced ? "still" : "fallback"}
      aria-hidden="true"
    >
      <div className="generation-fallback" />
      <canvas ref={canvasRef} />
    </div>
  );
}

export function GeneratingPlaceholder() {
  return (
    <GenerationBox active className="generation-placeholder">
      <LoadingSkeleton label="" layout="messages" rows={2} />
    </GenerationBox>
  );
}
