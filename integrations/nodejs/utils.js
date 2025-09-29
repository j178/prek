const detectLibc = require("detect-libc");

function getPlatformPackage() {
  let platform = process.platform;
  let arch = process.arch;

  let libc = "";
  if (platform === "linux") {
    libc = detectLibc.isNonGlibcLinuxSync() ? "musl" : "gnu";
  }

  // Map to our package naming conventions
  if (platform === "darwin") {
    if (arch === "arm64") {
      return {
        name: "prek-aarch64-apple-darwin",
        ext: "tar.gz",
      };
    } else if (arch === "x64") {
      return {
        name: "prek-x86_64-apple-darwin",
        ext: "tar.gz",
      };
    }
  } else if (platform === "win32") {
    if (arch === "arm64") {
      return {
        name: "prek-aarch64-pc-windows-msvc",
        ext: "zip",
      };
    } else if (arch === "ia32") {
      return {
        name: "prek-i686-pc-windows-msvc",
        ext: "zip",
      };
    } else if (arch === "x64") {
      return {
        name: "prek-x86_64-pc-windows-msvc",
        ext: "zip",
      };
    }
  } else if (platform === "linux") {
    if (arch === "x64") {
      return {
        name: `prek-x86_64-unknown-linux-${libc}`,
        ext: "tar.gz",
      };
    } else if (arch === "ia32") {
      return {
        name: `prek-i686-unknown-linux-${libc}`,
        ext: "tar.gz",
      };
    } else if (arch === "arm64") {
      return {
        name: `prek-aarch64-unknown-linux-${libc}`,
        ext: "tar.gz",
      };
    } else if (arch === "arm") {
      return {
        name: `prek-armv7-unknown-linux-${libc}eabihf`,
        ext: "tar.gz",
      };
    } else if (arch === "ppc64") {
      return {
        name: "prek-powerpc64-unknown-linux-gnu",
        ext: "tar.gz",
      };
    } else if (arch === "ppc64le") {
      return {
        name: "prek-powerpc64le-unknown-linux-gnu",
        ext: "tar.gz",
      };
    } else if (arch === "riscv64") {
      return {
        name: "prek-riscv64gc-unknown-linux-gnu",
        ext: "tar.gz",
      };
    } else if (arch === "s390x") {
      return {
        name: "prek-s390x-unknown-linux-gnu",
        ext: "tar.gz",
      };
    }
  }
}

module.exports = { getPlatformPackage };
