#!/usr/bin/env node

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");
const process = require("process");
const { getPlatformPackage } = require("./utils");

const isWin = process.platform === "win32";
const binName = isWin ? "prek.exe" : "prek";
const package = getPlatformPackage();
if (package === null) {
  console.error(
    `Unsupported platform and/or architecture ${process.platform} ${process.arch} for prek binary on npm.`
  );
  process.exit(1);
}
const binPath = path.resolve(
  __dirname,
  `./node_modules/${package.name}/`,
  binName
);

if (!fs.existsSync(binPath)) {
  console.error(`Error: prek not found at ${binPath}`);
  process.exit(1);
}

const child = spawn(binPath, process.argv.slice(2), {
  stdio: "inherit",
});

child.on("close", (code) => process.exit(code));
child.on("error", (err) => {
  console.error(err);
  process.exit(1);
});
