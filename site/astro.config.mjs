import fumadocs from "@fumadocs/astro";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import { validateBase } from "./scripts/content.mjs";

export default defineConfig({
  site: "https://androlax2.github.io",
  base: validateBase(process.env.COCKPIT_DOCS_BASE ?? "/agent-cockpit/latest/"),
  output: "static",
  trailingSlash: "always",
  integrations: [fumadocs({ components: "./src/components/mdx.tsx" })],
  vite: { plugins: [tailwindcss()] },
});
