import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./theme/tokens.css";
import "./theme/base.css";
import App from "./App";

// Dev-only mock backend (`npm run dev:mock`): the banner marks it. `__OXL_MOCK__` is false in every other build.
if (__OXL_MOCK__) void import("./mock/banner").then((m) => m.mountMockBanner());

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
