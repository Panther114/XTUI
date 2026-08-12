#!/usr/bin/env node

const path = require("node:path");
const { spawn } = require("node:child_process");

const executable = path.resolve(__dirname, "..", "xtui.exe");
const child = spawn(executable, process.argv.slice(2), {
  cwd: process.cwd(),
  stdio: "inherit",
  windowsHide: false,
});

child.on("error", (error) => {
  console.error(`Unable to start XTUI at ${executable}: ${error.message}`);
  process.exitCode = 1;
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exitCode = code ?? 1;
  }
});
