import { linkSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { binary, binDir, built, harnesses, holdForeground, resetDaemon } from "./helpers.ts";

// The launcher makes the run's directory and points the run at an oxd socket
// in it. The app it launches, the oxd the app starts, and the workers it
// starts inherit both.
process.env.OX_E2E_DIR ??= mkdtempSync(join(tmpdir(), "ox-e2e-"));
process.env.OX_SOCKET = join(process.env.OX_E2E_DIR, "oxd.sock");
// oxd reads workflow definitions from `config/ox/workflows` in the run's
// directory.
process.env.XDG_CONFIG_HOME = join(process.env.OX_E2E_DIR, "config");
// The app reads its `PATH` from the login shell at startup. This stub shell
// runs the app's command with the environment it inherits, so the app keeps
// the `PATH` below: the system directories and a directory of harness stubs.
const loginShell = join(process.env.OX_E2E_DIR, "login-shell");

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1,
  framework: "mocha",
  reporters: ["spec"],
  logLevel: "warn",
  waitforTimeout: 10_000,
  mochaOpts: { ui: "bdd", timeout: 60_000 },
  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath: binary("ox"),
        driverProvider: "embedded",
        captureBackendLogs: true,
        env: { PATH: `${harnesses()}:/usr/bin:/bin:/usr/sbin:/sbin`, SHELL: loginShell },
      },
    ],
  ],
  capabilities: [{ browserName: "tauri" }],

  // The app and every oxd of the run start from `binDir()`: hard links to
  // the binaries just built, with the fake agent as `pi-acp`, so workflow
  // steps on the `pi` harness run the fake agent while the app's sessions
  // run the Claude bridge. `harnesses/` holds a `claude` stub, so the app
  // offers Claude.
  onPrepare() {
    rmSync(binDir(), { recursive: true, force: true });
    mkdirSync(binDir(), { recursive: true });
    for (const name of ["ox", "oxd", "ox-mcp", "oxctl", "claude-code-acp", "oxd-fake-agent"]) {
      linkSync(built(name), binary(name));
    }
    linkSync(built("oxd-fake-agent"), binary("pi-acp"));
    mkdirSync(harnesses(), { recursive: true });
    writeFileSync(join(harnesses(), "claude"), "#!/bin/sh\n", { mode: 0o755 });
    writeFileSync(loginShell, '#!/bin/sh\nexec /bin/sh -c "$2"\n', { mode: 0o755 });
  },

  // The webview loads the UI after the WebDriver session starts; each spec
  // begins once the sidebar has rendered, with the window held in front.
  // Choosing the app's one window explicitly turns off the service's window
  // focus check before each command, which asks tauri-plugin-wdio, a plugin
  // the app does not have.
  async before() {
    await browser.switchToWindow(await browser.getWindowHandle());
    await $('button[aria-label="Add Workspace"]').waitForDisplayed({ timeout: 30_000 });
    await holdForeground();
  },

  // Each test starts with no runs, sessions, workspaces or workflows. Once the app exits,
  // oxd has no clients and no running agents, and exits after its idle period.
  async afterTest() {
    await resetDaemon();
  },

  onComplete() {
    console.log(`End-to-end run files: ${process.env.OX_E2E_DIR}`);
  },
};
