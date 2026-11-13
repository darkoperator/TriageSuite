// The key strip: only the keys that are live in this step.
import { Fragment } from "react";
import { KEY_STRIP } from "./keys";

export default function KeyStrip() {
  return (
    <div className="key-strip" aria-label="Keys">
      {KEY_STRIP.map(([key, what], ix) => (
        <Fragment key={key}>
          {ix > 0 && " · "}
          <kbd>{key}</kbd>
          {` ${what}`}
        </Fragment>
      ))}
    </div>
  );
}
