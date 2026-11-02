import { getCollection } from "astro:content";
import { createSearchAPI } from "fumadocs-core/search/server";

const pages = await getCollection("docs");
const search = createSearchAPI("simple", {
  indexes: pages.map((page) => ({
    title: page.data.title,
    url: "/",
    content: page.body ?? "",
  })),
});

export const GET = search.staticGET;
