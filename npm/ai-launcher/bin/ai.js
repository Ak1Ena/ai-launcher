#!/usr/bin/env node
const { spawn } = require("child_process");
const os = require("os");

const path = require("path");
const fs = require("fs");

const plat = `${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "ai-launcher.exe" : "ai-launcher";
const bin = path.join(__dirname, plat, exe);
if (!fs.existsSync(bin)) {
  console.error(`ai-launcher: no binary for ${plat}. Supported: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64.`);
  process.exit(1);
}

const child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });
const fwd = (sig) => () => { try { child.kill(sig); } catch {} };
const sigs = ["SIGINT", "SIGTERM", "SIGHUP"];
for (const s of sigs) process.on(s, fwd(s));

child.on("error", (e) => { console.error(`ai-launcher: ${e.message}`); process.exit(1); });
child.on("exit", (code, signal) => {
  if (signal) {
    for (const s of sigs) process.removeAllListeners(s);
    process.kill(process.pid, signal);
    // signal may not terminate us (e.g. handled); fall back to shell convention
    setTimeout(() => process.exit(128 + (os.constants.signals[signal] || 0)), 50);
  } else {
    process.exit(code ?? 1);
  }
});
