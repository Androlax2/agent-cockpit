import { getCollection } from "astro:content";
import { toFumadocsSource } from "@fumadocs/astro/source";
import { loader } from "fumadocs-core/source";
import manifest from "../../.source-manifest.json";

export { manifest };
export const source = loader({
  source: toFumadocsSource(await getCollection("docs"), await getCollection("meta")),
  baseUrl: manifest.base.replace(/\/$/, ""),
});

export const tree = await source.serializePageTree(source.getPageTree());
