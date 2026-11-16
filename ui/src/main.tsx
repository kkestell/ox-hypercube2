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

/** The animations of a running dot. */
const breathing = new Set(["breathe", "glow"]);

// Every running dot breathes in step: each one's animations count from the
// document timeline's start, whenever the dot began running.
document.addEventListener("animationstart", (event) => {
  if (!breathing.has(event.animationName) || !(event.target instanceof Element)) return;
  for (const animation of event.target.getAnimations({ subtree: true })) {
    if (animation instanceof CSSAnimation && breathing.has(animation.animationName)) {
      animation.startTime = 0;
    }
  }
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
