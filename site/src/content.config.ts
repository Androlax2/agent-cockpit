import { defineCollection } from "astro:content";
import { metaLoader, pageLoader } from "@fumadocs/astro/loaders";
import { pageSchema } from "fumadocs-core/source/schema";
import { z } from "zod";

export const collections = {
  docs: defineCollection({
    loader: pageLoader(),
    schema: pageSchema.extend({
      sourcePath: z.string(),
      sourceRevision: z.string(),
      sourceHash: z.string(),
      generated: z.boolean(),
    }),
  }),
  meta: defineCollection({ loader: metaLoader() }),
};
