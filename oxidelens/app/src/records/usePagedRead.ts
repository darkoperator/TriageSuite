// A list read a page at a time, by the late-answer rules of `useCaseRead`: *Show more* (`showMore`) asks for one page
// more of the same list and adds it to what is shown; another `base` starts again from one page. While the case is as
// it was when the pages were read, only the next page is read; after a write every page shown is read again, one after
// another, and the last answer stays up meanwhile. Nothing stops silently: `truncated` says more follow.
import { useCallback, useState } from "react";
import type { Listing } from "../contract/generated/Listing";
import { useCaseRead, type ReadState } from "./useCaseRead";

export interface Paged<T> {
  items: T[];
  /** More follow the pages shown. */
  truncated: boolean;
  /** How many pages `items` holds. */
  pages: number;
  /** The case's write generation they were read at. */
  generation: number;
}

export interface PagedRead<T> {
  /** The pages shown for `base`, or the read's error; null while there is none. */
  read: ReadState<Paged<T>> | null;
  /** Asks for one page more. */
  showMore(): void;
}

/**
 * Reads `page(limit, offset, queryId)` for `base` (null reads nothing) at the case's write `generation`, `size` items a
 * page. A failed cancel goes to `report` unless it is stale.
 */
export function usePagedRead<T>(
  base: string | null,
  generation: number,
  size: number,
  page: (limit: number, offset: number, queryId: string) => Promise<Listing<T>>,
  report: (e: unknown) => void,
): PagedRead<T> {
  const [pages, setPages] = useState<{ base: string | null; n: number }>({ base, n: 1 });
  const shownPages = pages.base === base ? pages.n : 1;
  if (pages.base !== base) setPages({ base, n: 1 });
  const read = useCaseRead<Paged<T>>(
    base,
    `${generation}:${shownPages}`,
    async (next, had) => {
      const kept = had !== undefined && had.generation === generation && had.truncated && had.pages < shownPages ? had : null;
      const items = kept === null ? [] : [...kept.items];
      for (let at = kept === null ? 0 : kept.pages; at < shownPages; at++) {
        const got = await page(size, at * size, next());
        items.push(...got.items);
        if (!got.truncated) return { items, truncated: false, pages: at + 1, generation };
      }
      return { items, truncated: true, pages: shownPages, generation };
    },
    report,
  );
  const shown = read?.value;
  const showMore = useCallback(() => {
    if (shown !== undefined && shown.truncated) setPages({ base, n: shown.pages + 1 });
  }, [shown, base]);
  return { read, showMore };
}
