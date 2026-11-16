import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/controls.css";
import "./styles/layout.css";
import "./styles/files.css";
import "./styles/sidebar.css";
import "./styles/sessions.css";
import "./styles/conversation.css";
import "./styles/composer.css";
import "./styles/run-view.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
