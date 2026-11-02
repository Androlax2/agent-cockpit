import { createSearchAPI } from "fumadocs-core/search/server";
import { source } from "../lib/source";

const search = createSearchAPI("simple", {
  indexes: await Promise.all(
    source.getPages().map(async (page) => ({
      title: page.data.title,
      url: page.url,
      content: await page.data.getText("raw"),
    })),
  ),
});
export const GET = search.staticGET;
