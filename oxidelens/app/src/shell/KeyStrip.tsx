// The key strip: only the keys that are live on the tab being shown.
import { Fragment } from "react";
import { keyStrip } from "./keys";
import type { Tab } from "./tabs";

export default function KeyStrip({ kind }: { kind: Tab["kind"] | null }) {
  return (
    <div className="key-strip" aria-label="Keys">
      {keyStrip(kind).map(([key, what], ix) => (
        <Fragment key={key}>
          {ix > 0 && " · "}
          <kbd>{key}</kbd>
          {` ${what}`}
        </Fragment>
      ))}
    </div>
  );
}
