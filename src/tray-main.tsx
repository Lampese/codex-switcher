import React from "react";
import ReactDOM from "react-dom/client";
import TrayMenu from "./TrayMenu";
import { syncThemeFromStorage } from "./lib/theme";
import { I18nProvider } from "./lib/i18n";
import "./App.css";

syncThemeFromStorage();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <TrayMenu />
    </I18nProvider>
  </React.StrictMode>
);
