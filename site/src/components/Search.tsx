import { useStaticSearch } from "fumadocs-core/search/client";
import {
  SearchDialog,
  SearchDialogClose,
  SearchDialogContent,
  SearchDialogFooter,
  SearchDialogHeader,
  SearchDialogIcon,
  SearchDialogInput,
  SearchDialogList,
  SearchDialogOverlay,
  type SharedProps,
} from "fumadocs-ui/components/dialog/search";
import { useRef } from "react";
import { base, version } from "../../.source-manifest.json";

export default function Search(props: SharedProps) {
  const search = useStaticSearch({ from: `${base}search.json` });
  const returnFocus = useRef<HTMLElement | null>(null);
  return (
    <SearchDialog {...search} {...props}>
      <SearchDialogOverlay />
      <SearchDialogContent
        onOpenAutoFocus={() => {
          returnFocus.current =
            document.activeElement instanceof HTMLElement ? document.activeElement : null;
        }}
        onCloseAutoFocus={(event) => {
          if (returnFocus.current?.isConnected) {
            event.preventDefault();
            returnFocus.current.focus();
          }
        }}
      >
        <SearchDialogHeader>
          <SearchDialogIcon />
          <SearchDialogInput aria-label="Search documentation" />
          <SearchDialogClose className="min-h-11 min-w-20 px-3" aria-label="Close search">
            Close
          </SearchDialogClose>
        </SearchDialogHeader>
        <p className="px-4 pt-3 text-sm text-fd-muted-foreground">
          Search in {version === "latest" ? "Latest (main)" : version}
        </p>
        {search.error ? (
          <div role="alert" className="p-4">
            Search is unavailable. Close search and use the navigation to browse this version.
          </div>
        ) : (
          <SearchDialogList
            Empty={() => (
              <p className="p-4">
                No matches. Try another term or close search to browse the navigation.
              </p>
            )}
          />
        )}
        <SearchDialogFooter>
          Use Close or Escape to return to browsing. Arrow keys and Enter select a result.
        </SearchDialogFooter>
      </SearchDialogContent>
    </SearchDialog>
  );
}
