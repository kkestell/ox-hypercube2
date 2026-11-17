import { linkSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { stopDaemon } from "./helpers.ts";

// The launcher, app and workers inherit a socket of their own, and a configuration
// home that keeps the user's global Git ignore and attributes files out.
const runDir = process.env.OX_E2E_DIR ?? mkdtempSync(join(tmpdir(), "ox-e2e-"));
process.env.OX_E2E_DIR = runDir;
process.env.OX_SOCKET = join(runDir, "oxd.sock");
process.env.XDG_CONFIG_HOME = join(runDir, "config");
const bin = join(runDir, "bin");
const loginShell = join(runDir, "login-shell");

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
        appBinaryPath: join(bin, "ox"),
        driverProvider: "embedded",
        captureBackendLogs: true,
        env: { PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`, SHELL: loginShell },
      },
    ],
  ],
  capabilities: [{ browserName: "tauri" }],

  onPrepare() {
    const target = resolve(import.meta.dirname, "../../target/debug");
    mkdirSync(bin);
    for (const name of ["ox", "oxd", "ox-mcp", "oxd-fake-agent"]) {
      linkSync(join(target, name), join(bin, name));
    }
    // The app detects Claude on its login shell's PATH. The fixture bridge
    // answers the controls probe and runs the scripted ACP agent for sessions.
    writeFileSync(join(bin, "claude"), "#!/bin/sh\n", { mode: 0o755 });
    writeFileSync(
      join(bin, "claude-code-acp"),
      `#!/bin/sh
if [ "$1" = --describe ]; then
  printf '%s\\n' '{"configOptions":[],"availableCommands":[]}'
  exit
fi
exec "$(dirname "$0")/oxd-fake-agent" "$@"
`,
      { mode: 0o755 },
    );
    writeFileSync(loginShell, '#!/bin/sh\nexec /bin/sh -c "$2"\n', { mode: 0o755 });
    // The service closes the app after the launcher's onComplete hook.
    process.once("exit", stopDaemon);
  },

  async before() {
    // Choose the app's window so the service skips per-command focus checks
    // that require tauri-plugin-wdio.
    await browser.switchToWindow(await browser.getWindowHandle());
    await $('button[aria-label="Add Workspace"]').waitForDisplayed({ timeout: 30_000 });
  },

  onComplete() {
    console.log(`End-to-end run files: ${runDir}`);
  },
};
