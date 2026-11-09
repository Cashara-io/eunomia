import type { MetadataRoute } from "next";

export default function manifest(): MetadataRoute.Manifest {
  return {
    name: "Eunomia",
    short_name: "Eunomia",
    description: "Shared memory for the AI apps you already use",
    start_url: "/",
    display: "standalone",
    background_color: "#FAFAF7",
    theme_color: "#171717",
    icons: [
      {
        src: "/icon1.png",
        sizes: "192x192",
        type: "image/png",
        purpose: "any",
      },
      {
        src: "/icon2.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "any",
      },
      {
        src: "/maskable-icon.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "maskable",
      },
    ],
  };
}
