import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans-arabic/400.css";
import "@fontsource/ibm-plex-sans-arabic/500.css";
import "@fontsource/ibm-plex-sans-arabic/600.css";
import "./styles.css";
import App from "./App";

async function start() {
  // Outside the native shell (plain `npm run dev`), fall back to canned data.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    (await import("./devMock")).installDevMock();
  }
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

start();
