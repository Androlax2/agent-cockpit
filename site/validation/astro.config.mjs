import fumadocs from "@fumadocs/astro";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";

export default defineConfig({
  output: "static",
  integrations: [fumadocs({ components: "./src/components/mdx.tsx" })],
  vite: { plugins: [tailwindcss()] },
});
