import { defineCollection } from "astro:content";
import { pageLoader } from "@fumadocs/astro/loaders";

export const collections = {
  docs: defineCollection({ loader: pageLoader() }),
};
