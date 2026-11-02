import { navigate } from "astro:transitions/client";
import { type SerializedPageTree, useFumadocsLoader } from "fumadocs-core/source/client";
import type { TOCItemType } from "fumadocs-core/toc";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { DocsDescription, DocsPage, DocsTitle, EditOnGitHub } from "fumadocs-ui/layouts/docs/page";
import { RootProvider } from "fumadocs-ui/provider/astro";
import type { ReactNode } from "react";
import Search from "./Search";

interface Props {
  tree: SerializedPageTree;
  pathname: string;
  params: Record<string, string | undefined>;
  title: string;
  description?: string;
  toc: TOCItemType[];
  base: string;
  version: string;
  versions: string[];
  sourcePath: string;
  revision: string;
  generated: boolean;
  children?: ReactNode;
}

export function Docs({
  tree,
  pathname,
  params,
  title,
  description,
  toc,
  base,
  version,
  versions,
  sourcePath,
  revision,
  generated,
  children,
}: Props) {
  const data = useFumadocsLoader({ tree });
  const versionRoot = base.replace(/[^/]+\/$/, "");
  return (
    <RootProvider
      pathname={pathname}
      params={params}
      navigate={navigate}
      search={{ SearchDialog: Search }}
    >
      <DocsLayout
        tree={data.tree}
        tabs={false}
        nav={{ title: "Torlek", url: `${base}get-started/install/` }}
        githubUrl="https://github.com/Androlax2/agent-cockpit"
        sidebar={{
          collapsible: false,
          banner: (
            <label className="mx-3 mb-3 flex flex-col gap-1 text-sm">
              <span className="text-fd-muted-foreground">Documentation version</span>
              <select
                aria-label="Documentation version"
                className="min-h-11 rounded-lg border bg-fd-background px-3"
                value={version}
                onChange={(event) =>
                  window.location.assign(`${versionRoot}${event.target.value}/get-started/install/`)
                }
              >
                {versions.map((value) => (
                  <option key={value} value={value}>
                    {value === "latest" ? "Latest (main)" : value}
                  </option>
                ))}
              </select>
            </label>
          ),
        }}
      >
        <DocsPage toc={toc}>
          <DocsTitle>{title}</DocsTitle>
          <DocsDescription>{description}</DocsDescription>
          {generated && (
            <p className="text-sm text-fd-muted-foreground">
              Generated reference · source revision {revision.slice(0, 12)}
            </p>
          )}
          {children}
          <div className="mt-8 flex flex-wrap items-center gap-3 text-sm text-fd-muted-foreground">
            <a href={`https://github.com/Androlax2/agent-cockpit/blob/${revision}/${sourcePath}`}>
              Source: {sourcePath} · {revision.slice(0, 12)}
            </a>
            <EditOnGitHub
              href={`https://github.com/Androlax2/agent-cockpit/edit/main/${sourcePath}`}
            >
              Edit current source on GitHub
            </EditOnGitHub>
          </div>
        </DocsPage>
      </DocsLayout>
    </RootProvider>
  );
}
