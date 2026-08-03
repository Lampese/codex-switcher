import React from "react";
import ReactDOM from "react-dom/client";
import TrayMenu from "./TrayMenu";
import { LanguageProvider } from "./lib/i18n";
import { syncThemeFromStorage } from "./lib/theme";
import "./App.css";

syncThemeFromStorage();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <LanguageProvider>
      <TrayMenu />
    </LanguageProvider>
  </React.StrictMode>
);
