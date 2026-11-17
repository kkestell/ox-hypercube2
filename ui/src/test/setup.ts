import "@testing-library/jest-dom/vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom lays nothing out, so it has no scrolling to do.
Element.prototype.scrollIntoView = () => {};
// jsdom has no modal dialogs.
HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
  this.open = true;
};
HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
  this.open = false;
};

afterEach(() => {
  cleanup();
  clearMocks();
  sessionStorage.clear();
  localStorage.clear();
});
