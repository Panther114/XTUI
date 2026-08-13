#!/usr/bin/env node

// XTUI launcher. Spawns the compiled Rust binary, downloading a prebuilt
// release on first use when no local build exists. Works from any Node
// 18+ runtime: `npx github:panther114/xtui`, `bunx github:panther114/xtui`,
// or a plain `npm link` / `bun link` local install.

const path = require("node:path");
const fs = require("node:fs");
const os = require("node:os");
const { spawnSync } = require("node:child_process");

const root = path.resolve(__dirname, "..");
const version = require(path.join(root, "package.json")).version;
const exeName = process.platform === "win32" ? "xtui.exe" : "xtui";
const localBinary = path.join(root, exeName);
const versionedBinary = path.join(root, "target", "versions", version, "release", exeName);
const builtBinary = path.join(root, "target", "release", exeName);
const cacheRoot = path.join(
  process.env.LOCALAPPDATA || path.join(os.homedir(), ".cache"),
  "xtui",
  "bin",
  version
);
const cachedBinary = path.join(cacheRoot, exeName);
const override = process.env.XTUI_BIN;
const candidates = [override, versionedBinary, builtBinary, localBinary, cachedBinary].filter(Boolean);
const executable = candidates.find((candidate) => fs.existsSync(candidate));

const RELEASE_BASE =
  "https://github.com/panther114/xtui/releases/latest/download";
const ARTIFACT = {
  win32: "xtui-windows-x86_64.exe",
  linux: "xtui-linux-x86_64",
  darwin: "xtui-macos-aarch64",
}[process.platform];

async function downloadBinary() {
  if (!ARTIFACT) {
    throw new Error(
      `no prebuilt XTUI for platform "${process.platform}"; build it with: cargo build --release`
    );
  }
  const url = `${RELEASE_BASE}/${ARTIFACT}`;
  process.stderr.write(`XTUI: downloading ${url}\n`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    throw new Error(`download failed (HTTP ${response.status})`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  fs.mkdirSync(cacheRoot, { recursive: true });
  const temporary = `${cachedBinary}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, bytes, { mode: 0o755 });
  fs.renameSync(temporary, cachedBinary);
  process.stderr.write(`XTUI: saved ${cachedBinary}\n`);
  return cachedBinary;
}

async function main() {
  const target = executable || (await downloadBinary());
  // Block the JavaScript launcher for the entire interactive session. XTUI is
  // the sole console owner until it restores cooked input and exits.
  const child = spawnSync(target, process.argv.slice(2), {
    cwd: process.cwd(),
    stdio: "inherit",
    windowsHide: false,
  });
  if (child.error) {
    throw new Error(
      `Unable to start XTUI at ${target}: ${child.error.message}\n` +
        `If the download is not available yet, build it locally with: cargo build --release`
    );
  }
  if (child.signal) process.kill(process.pid, child.signal);
  process.exitCode = child.status ?? 1;
}

main().catch((error) => {
  console.error(`XTUI: ${error.message}`);
  console.error(
    "Install Rust from https://rustup.rs, then run in the xtui checkout: cargo build --release"
  );
  process.exitCode = 1;
});
