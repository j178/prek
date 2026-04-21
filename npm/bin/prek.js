#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");
const { createRequire } = require("module");

const DEBUG = process.env.PREK_DEBUG === "1";
const PLATFORMS = require("../platforms.json");

function debug(message) {
  if (DEBUG) {
    console.error(`[prek-debug] ${message}`);
  }
}

function isMusl() {
  if (process.platform !== "linux") {
    return false;
  }

  if (fs.existsSync("/etc/alpine-release")) {
    debug("detected musl via /etc/alpine-release");
    return true;
  }

  const report = process.report?.getReport?.();
  if (report?.header?.glibcVersionRuntime) {
    debug(`detected glibc ${report.header.glibcVersionRuntime}`);
    return false;
  }
  if (report?.sharedObjects?.some((entry) => entry.includes("musl"))) {
    debug("detected musl via process.report shared objects");
    return true;
  }

  try {
    const result = spawnSync("ldd", ["--version"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.toLowerCase();
    const musl = output.includes("musl");
    debug(`detected ${musl ? "musl" : "glibc"} via ldd`);
    return musl;
  } catch {
    debug("failed to run ldd, assuming glibc");
  }

  try {
    for (const libDir of ["/lib", "/lib64"]) {
      if (!fs.existsSync(libDir)) {
        continue;
      }

      const files = fs.readdirSync(libDir);
      if (
        files.some(
          (file) => file.startsWith("ld-musl-") || file.startsWith("libc.musl-"),
        )
      ) {
        debug(`detected musl via ${libDir}`);
        return true;
      }
    }
  } catch {
    debug("failed to scan /lib and /lib64 for musl");
  }

  const ldLibraryPath = process.env.LD_LIBRARY_PATH ?? "";
  if (ldLibraryPath.toLowerCase().includes("musl")) {
    debug("detected musl via LD_LIBRARY_PATH");
    return true;
  }

  return false;
}

function armVersion() {
  const version = Number(process.config.variables.arm_version ?? 0);
  return Number.isFinite(version) ? version : 0;
}

function matchesArmVersion(spec) {
  const version = armVersion();
  if (spec.armVersionMin != null && version < spec.armVersionMin) {
    return false;
  }
  if (spec.armVersionMax != null && version > spec.armVersionMax) {
    return false;
  }
  return true;
}

function currentLibc() {
  if (process.platform !== "linux") {
    return null;
  }
  return isMusl() ? "musl" : "glibc";
}

function pickPlatformPackage() {
  const libc = currentLibc();
  debug(
    `selecting package for platform=${process.platform} arch=${process.arch}` +
      `${libc ? ` libc=${libc}` : ""} armVersion=${armVersion()}`,
  );

  const candidates = PLATFORMS.filter((spec) => {
    if (!spec.os.includes(process.platform)) {
      return false;
    }
    if (!spec.cpu.includes(process.arch)) {
      return false;
    }
    if (spec.libc && spec.libc !== libc) {
      return false;
    }
    if (!matchesArmVersion(spec)) {
      return false;
    }
    return true;
  }).sort((left, right) => {
    const leftSpecificity = left.armVersionMin ?? 0;
    const rightSpecificity = right.armVersionMin ?? 0;
    return rightSpecificity - leftSpecificity;
  });

  if (candidates.length === 0) {
    const libcSuffix = libc ? ` (${libc})` : "";
    throw new Error(
      `Unsupported platform: ${process.platform} ${process.arch}${libcSuffix}`,
    );
  }

  return candidates[0];
}

function resolvePackageJson(packageName) {
  const specifier = `${packageName}/package.json`;
  const attempts = [];

  const resolvers = [
    () => createRequire(__filename).resolve(specifier),
    () =>
      path.resolve(
        __dirname,
        "..",
        "..",
        packageName.split("/")[1],
        "package.json",
      ),
  ];

  for (const resolve of resolvers) {
    try {
      const packageJsonPath = resolve();
      attempts.push(packageJsonPath);
      debug(`resolved ${specifier} candidate: ${packageJsonPath}`);
      if (fs.existsSync(packageJsonPath)) {
        debug(`using ${specifier}: ${packageJsonPath}`);
        return packageJsonPath;
      }
      debug(`candidate does not exist: ${packageJsonPath}`);
    } catch (error) {
      attempts.push(String(error.message ?? error));
      debug(`failed to resolve ${specifier}: ${error.message ?? error}`);
    }
  }

  debug(`resolution attempts for ${packageName}: ${attempts.join(" | ")}`);
  throw new Error(
    `Platform package ${packageName} is not installed. Reinstall @j178/prek for this platform.`,
  );
}

function ensureExecutable(binaryPath) {
  if (process.platform === "win32") {
    return;
  }

  try {
    fs.accessSync(binaryPath, fs.constants.X_OK);
  } catch {
    fs.chmodSync(binaryPath, 0o755);
  }
}

function main() {
  if (process.env.PREK_BINARY) {
    const override = path.resolve(process.env.PREK_BINARY);
    debug(`using PREK_BINARY override: ${override}`);
    if (!fs.existsSync(override)) {
      throw new Error(`PREK_BINARY does not exist: ${override}`);
    }
    ensureExecutable(override);
    runBinary(override);
    return;
  }

  const spec = pickPlatformPackage();
  debug(`selected package ${spec.packageName}`);

  const packageJsonPath = resolvePackageJson(spec.packageName);
  const binaryPath = path.join(path.dirname(packageJsonPath), spec.binaryName);
  debug(`selected binary ${binaryPath}`);
  ensureExecutable(binaryPath);
  runBinary(binaryPath);
}

function runBinary(binaryPath) {
  debug(`running ${binaryPath} ${process.argv.slice(2).join(" ")}`);
  const result = spawnSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
  });

  if (result.error) {
    throw result.error;
  }

  if (result.signal) {
    process.kill(process.pid, result.signal);
    return;
  }

  process.exit(result.status ?? 1);
}

try {
  main();
} catch (error) {
  console.error(`prek: ${error.message}`);
  process.exit(1);
}
