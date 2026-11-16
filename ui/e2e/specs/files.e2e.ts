import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { deleteSessions, openSession, startSession } from "../helpers.ts";

/** The right sidebar's row for the entry `name`. */
function treeRow(name: string) {
  return $(
    `//aside[@class="panel right"]//div[@class="sidebar-row tree-row"][.//span[@class="title"][.="${name}"]]`,
  );
}

describe("The file tree", () => {
  it("shows the viewed session's folder live and keeps its open folders", async () => {
    const cwd = await startSession();
    await writeFile(join(cwd, "a.txt"), "a\n");
    await mkdir(join(cwd, "sub"));
    await mkdir(join(cwd, "sub", "nested"));
    await writeFile(join(cwd, "sub", "inner.txt"), "inner\n");
    const other = await startSession();
    await openSession(cwd);
    await expect(treeRow("sub")).toBeDisplayed();
    await expect(treeRow("a.txt")).toBeDisplayed();
    await expect(treeRow("sub").$("svg.lucide-folder")).toBeDisplayed();
    const rootIconX = await treeRow("sub").$("svg").getLocation("x");
    expect(await treeRow("a.txt").$("svg").getLocation("x")).toBe(rootIconX);

    await writeFile(join(cwd, "b.txt"), "b\n");
    await expect(treeRow("b.txt")).toBeDisplayed();

    const sub = treeRow("sub").$("button.select");
    await sub.click();
    await expect(sub).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();
    await expect(sub.$("svg.lucide-folder-open")).toBeDisplayed();
    await expect(treeRow("nested")).toBeDisplayed();
    const childIconX = await treeRow("nested").$("svg").getLocation("x");
    expect(await treeRow("inner.txt").$("svg").getLocation("x")).toBe(childIconX);
    expect(childIconX).toBeGreaterThan(rootIconX);

    await openSession(other);
    await expect(treeRow("sub")).not.toBeExisting();
    await openSession(cwd);
    await expect(treeRow("sub").$("button.select")).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();
  });
});

describe("The right sidebar's tabs", () => {
  it("shows Files first, and Changes in its place", async () => {
    const cwd = await startSession();
    await writeFile(join(cwd, "a.txt"), "a\n");
    await openSession(cwd);
    const files = $('aside.right [role="tab"][aria-label="Files"]');
    const changes = $('aside.right [role="tab"][aria-label="Changes"]');
    await expect(files).toHaveAttribute("aria-selected", "true");
    await expect(treeRow("a.txt")).toBeDisplayed();

    await changes.click();
    await expect(changes).toHaveAttribute("aria-selected", "true");
    await expect(files).toHaveAttribute("aria-selected", "false");
    await expect($('aside.right [role="tabpanel"][aria-label="Changes"]')).toBeExisting();
    await expect(treeRow("a.txt")).not.toBeExisting();

    await files.click();
    await expect(treeRow("a.txt")).toBeDisplayed();
  });
});

describe("The panel toggles", () => {
  it("end the rightmost header", async () => {
    const cwd = await startSession();
    await writeFile(join(cwd, "a.txt"), "a\n");
    await openSession(cwd);
    const toggleRight = () => $('button[aria-label="Toggle Secondary Side Bar"]').click();
    await expect($("aside.right .toggles")).toBeExisting();
    await expect($(".content-header .toggles")).not.toBeExisting();

    await treeRow("a.txt").$("button.select").click();
    await expect($(".file-header")).toBeDisplayed();
    await expect($(".file-header .toggles")).not.toBeExisting();

    await toggleRight();
    await expect($(".file-header .toggles")).toBeExisting();
    await expect($(".content-header .toggles")).not.toBeExisting();

    await $('button[aria-label="Close file"]').click();
    await expect($(".content-header .toggles")).toBeExisting();

    await toggleRight();
    await expect($("aside.right .toggles")).toBeExisting();
    await expect($(".content-header .toggles")).not.toBeExisting();
  });
});

describe("The file viewer", () => {
  it("opens nested plain text, follows disk edits, and restores each session before paint", async () => {
    const cwd = await startSession();
    const text = Array.from(
      { length: 160 },
      (_, index) => `${index} ${"long line ".repeat(40)}`,
    ).join("\n");
    await mkdir(join(cwd, "sub"));
    await writeFile(join(cwd, "sub", "README.md"), text);
    await writeFile(join(cwd, "second.txt"), "second");
    const other = await startSession();
    await writeFile(join(other, "other.txt"), text.replaceAll("long line", "other text"));
    const empty = await startSession();
    await openSession(cwd);
    await treeRow("sub").$("button.select").click();
    await treeRow("README.md").$("button.select").click();
    await browser.waitUntil(async () =>
      browser.execute(
        (expected) => document.querySelector(".file-body pre")?.textContent === expected,
        text,
      ),
    );
    await expect($(".file-pill")).toHaveText("README.md");
    expect(
      await browser.execute(
        () => getComputedStyle(document.querySelector(".file-body pre")!).whiteSpace,
      ),
    ).toBe("pre");

    // The embedded driver dispatches synthetic events and does not implement native
    // pointer capture. Substitute that boundary while exercising the real handlers.
    const drag = async (fraction: number) => {
      await browser.execute((fraction) => {
        const split = document.querySelector(".session-content")!.getBoundingClientRect();
        const sash = document.querySelector<HTMLElement>(".file-sash")!;
        const bounds = sash.getBoundingClientRect();
        const capture = sash.setPointerCapture;
        sash.setPointerCapture = () => {};
        try {
          const init = {
            pointerId: 1,
            button: 0,
            bubbles: true,
            clientY: bounds.y + bounds.height / 2,
          };
          sash.dispatchEvent(new PointerEvent("pointerdown", { ...init, clientX: bounds.x }));
          sash.dispatchEvent(
            new PointerEvent("pointermove", {
              ...init,
              clientX: split.left + split.width * fraction,
            }),
          );
          sash.dispatchEvent(new PointerEvent("lostpointercapture", init));
        } finally {
          sash.setPointerCapture = capture;
        }
      }, fraction);
    };
    const ratio = () =>
      browser.execute(() => {
        const chat = document.querySelector(".chat-pane")!.getBoundingClientRect().width;
        const file = document.querySelector(".file-pane")!.getBoundingClientRect().width;
        return chat / (chat + file);
      });
    await drag(0.01);
    expect(await ratio()).toBeCloseTo(0.25, 2);
    await drag(0.99);
    expect(await ratio()).toBeCloseTo(0.75, 2);
    await drag(0.4);
    const savedRatio = await ratio();
    await browser.execute(() => {
      const body = document.querySelector(".file-body")!;
      body.scrollTop = 400;
      body.scrollLeft = 200;
      body.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await openSession(other);
    await treeRow("other.txt").$("button.select").click();
    await expect($(".file-body pre")).toHaveText(expect.stringContaining("other text"));
    await drag(0.6);
    const otherRatio = await ratio();
    await browser.execute(() => {
      const body = document.querySelector(".file-body")!;
      body.scrollTop = 220;
      body.scrollLeft = 80;
      body.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    const updated = `updated\n${text}`;
    await writeFile(join(cwd, "sub", "README.md"), updated);
    // Observe a live filesystem notification while the target viewer is inactive.
    await writeFile(join(other, "other.txt"), `active update\n${text}`);
    await browser.waitUntil(async () =>
      browser.execute(
        () =>
          document.querySelector(".file-body pre")?.textContent?.startsWith("active update") ===
          true,
      ),
    );

    const firstFrame = async (
      target: string,
    ): Promise<{ text: string | null; ratio: number; top: number; left: number }> =>
      browser.executeAsync((target, done) => {
        const button = [
          ...document.querySelectorAll<HTMLButtonElement>(".session > button.select"),
        ].find((button) => button.title.replace(/\/$/, "") === target)!;
        button.click();
        requestAnimationFrame(() => {
          const chat = document.querySelector(".chat-pane")?.getBoundingClientRect().width ?? 0;
          const file = document.querySelector(".file-pane")?.getBoundingClientRect().width ?? 0;
          const body = document.querySelector(".file-body");
          done({
            text: document.querySelector(".file-body pre")?.textContent ?? null,
            ratio: chat / (chat + file),
            top: body?.scrollTop ?? 0,
            left: body?.scrollLeft ?? 0,
          });
        });
      }, target) as Promise<{ text: string | null; ratio: number; top: number; left: number }>;
    // Each file's watcher notifies on its own, so the README's edit can reach
    // its inactive viewer after the active viewer's edit. Each try selects
    // `cwd` from `other`, and so observes the first frame of a selection.
    let restored = await firstFrame(cwd);
    await browser.waitUntil(async () => {
      if (restored.text !== updated) {
        await firstFrame(other);
        restored = await firstFrame(cwd);
      }
      return restored.text === updated;
    });
    expect(restored.text).toBe(updated);
    expect(restored.ratio).toBeCloseTo(savedRatio, 2);
    expect(restored.top).toBe(400);
    expect(restored.left).toBe(200);
    const otherFrame = await firstFrame(other);
    expect(otherFrame.text).toBe(`active update\n${text}`);
    expect(otherFrame.ratio).toBeCloseTo(otherRatio, 2);
    expect(otherFrame.top).toBe(220);
    expect(otherFrame.left).toBe(80);
    expect((await firstFrame(empty)).text).toBeNull();
    await expect($(".file-pane")).not.toBeExisting();

    await openSession(cwd);
    await $('button[aria-label="Toggle Secondary Side Bar"]').click();
    expect(await ratio()).toBeCloseTo(savedRatio, 2);
    await $('button[aria-label="Toggle Secondary Side Bar"]').click();
    const windowSize = await browser.getWindowSize();
    const inner = await browser.execute(() => ({
      width: window.innerWidth,
      height: window.innerHeight,
    }));
    await browser.setWindowSize(900, 650);
    // The webview takes the window's new size, less the window's frame, after
    // the window has it.
    await browser.waitUntil(() =>
      browser.execute(
        (width, height) => window.innerWidth === width && window.innerHeight === height,
        inner.width - (windowSize.width - 900),
        inner.height - (windowSize.height - 650),
      ),
    );
    expect(await ratio()).toBeCloseTo(savedRatio, 2);
    expect(
      await browser.execute(() => {
        const chat = document.querySelector(".chat-pane")!.getBoundingClientRect();
        const composer = document.querySelector(".composer")!.getBoundingClientRect();
        const viewer = document.querySelector(".file-pane")!.getBoundingClientRect();
        const chatHeader = document.querySelector(".content-header")!.getBoundingClientRect();
        const fileHeader = document.querySelector(".file-header")!.getBoundingClientRect();
        return (
          composer.right <= chat.right &&
          composer.bottom <= chat.bottom &&
          viewer.left >= chat.right &&
          chatHeader.y + chatHeader.height / 2 === fileHeader.y + fileHeader.height / 2
        );
      }),
    ).toBe(true);
    await browser.setWindowSize(windowSize.width, windowSize.height);
    await treeRow("second.txt").$("button.select").click();
    await expect($(".file-body pre")).toHaveText("second");
    await writeFile(join(cwd, "sub", "README.md"), "old file changed");
    await writeFile(join(cwd, "second.txt"), "new file changed");
    await expect($(".file-body pre")).toHaveText("new file changed");
    await browser.saveScreenshot("/tmp/ox-file-viewer.png");
    // Observe real IPC cleanup without replacing the commands themselves.
    await browser.execute(() => {
      const observed = { files: [] as number[], unwatched: [] as number[], fetch: window.fetch };
      (window as unknown as { fileUnwatch: typeof observed }).fileUnwatch = observed;
      window.fetch = (input, options) => {
        const url = decodeURIComponent(String(input));
        const body = typeof options?.body === "string" ? options.body : "";
        if (url.endsWith("/watch_file")) {
          observed.files.push(Number(/__CHANNEL__:(\d+)/.exec(body)?.[1]));
        }
        if (url.endsWith("/unwatch")) observed.unwatched.push(JSON.parse(body).channelId);
        return observed.fetch.call(window, input, options);
      };
    });
    type Observed = { fileUnwatch: { files: number[]; unwatched: number[] } };
    try {
      await $('button[aria-label="Close file"]').click();
      await expect($(".file-pane")).not.toBeExisting();
      await writeFile(join(cwd, "second.txt"), "closed edit");
      await treeRow("second.txt").$("button.select").click();
      await expect($(".file-body pre")).toHaveText("closed edit");
      expect(await ratio()).toBeCloseTo(savedRatio, 2);
      await browser.waitUntil(async () =>
        browser.execute(() => (window as unknown as Observed).fileUnwatch.unwatched.length === 1),
      );
      await deleteSessions(cwd);
      await browser.waitUntil(async () =>
        browser.execute(() => {
          const { files, unwatched } = (window as unknown as Observed).fileUnwatch;
          return files.length === 1 && files.every((id) => unwatched.includes(id));
        }),
      );
      await expect($(".file-pane")).not.toBeExisting();
    } finally {
      await browser.execute(() => {
        window.fetch = (
          window as unknown as { fileUnwatch: { fetch: typeof fetch } }
        ).fileUnwatch.fetch;
      });
    }
  });
});
