import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";
import { initializeI18n } from "./i18n";

async function renderApp() {
  await initializeI18n();
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
}

void renderApp();
