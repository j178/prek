#!/usr/bin/env node

/**
 * prek postinstall script
 *
 * This script ensures that the correct binary for the current platform and
 * architecture is downloaded and available.
 */

const fs = require("fs");
const path = require("path");
const https = require("https");
const packageJson = require("./package.json");
const process = require("process");
const { getPlatformPackage } = require("./utils");

const VERSION = packageJson.version;

// Extract all files from a tarball to a destination directory
async function extractTarball(tarballStream, destDir) {
  const { extract } = require("tar");

  if (!fs.existsSync(destDir)) {
    fs.mkdirSync(destDir, { recursive: true });
  }

  return new Promise((resolve, reject) => {
    tarballStream
      .pipe(extract({ cwd: destDir, strip: 1 }))
      .on("error", (err) => reject(err))
      .on("end", () => resolve());
  });
}

// Extract all files from a zip to a destination directory
async function extractZip(zipStream, destDir) {
  const unzipper = require("unzipper");

  if (!fs.existsSync(destDir)) {
    fs.mkdirSync(destDir, { recursive: true });
  }

  return new Promise((resolve, reject) => {
    zipStream
      .pipe(unzipper.Extract({ path: destDir }))
      .on("error", (err) => reject(err))
      .on("close", () => resolve());
  });
}

async function downloadAndExtractBinary(packageName, extension) {
  let packageUrl = `https://github.com/j178/prek/releases/download/v${VERSION}/${packageName}.${extension}`;
  process.stdout.write(`Downloading ${packageUrl}...\n`);

  return new Promise((resolve) => {
    https
      .get(packageUrl, (response) => {
        if (response.statusCode === 302 || response.statusCode === 301) {
          // Handle redirects
          https
            .get(response.headers.location, handleResponse)
            .on("error", (err) => {
              console.error("Download error:", err);
              resolve();
            });
          return;
        }

        handleResponse(response);

        async function handleResponse(response) {
          try {
            if (response.statusCode !== 200) {
              throw new Error(
                `Download failed with status code: ${response.statusCode}`
              );
            }

            const destDir = path.join(__dirname, "node_modules", packageName);
            if (extension === "tar.gz") {
              await extractTarball(response, destDir);
            } else if (extension === "zip") {
              await extractZip(response, destDir);
            } else {
              throw new Error(`Unsupported archive format: ${extension}`);
            }

            process.stdout.write(
              `Successfully downloaded and installed ${packageName}\n`
            );
          } catch (error) {
            console.error("Error during extraction:", error);
            resolve();
          } finally {
            resolve();
          }
        }
      })
      .on("error", (err) => {
        console.error("Download error:", err);
        resolve();
      });
  });
}

async function main() {
  try {
    let package = getPlatformPackage();
    if (package === null) {
      console.error(
        `Unsupported platform and/or architecture ${process.platform} ${process.arch} for prek binary on npm.`
      );
      process.exit(1);
    } else {
      const { name, ext } = package;
      await downloadAndExtractBinary(name, ext);
      process.exit(0);
    }
  } catch (error) {
    console.error(error);
    return;
  }
}

main();
