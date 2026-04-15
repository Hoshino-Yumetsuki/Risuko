#!/usr/bin/env node

/**
 * risuko-app — Downloads and launches the Risuko desktop app.
 *
 * On first run for a given version, the platform-appropriate release asset is
 * downloaded from GitHub Releases and cached locally. Subsequent runs use the
 * cached binary directly.
 *
 * Cache locations:
 *   macOS:   ~/Library/Application Support/risuko-launcher/<version>/
 *   Linux:   $XDG_DATA_HOME/risuko-launcher/<version>/  (default ~/.local/share/…)
 *   Windows: %APPDATA%\risuko-launcher\<version>\
 */

const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const { execFileSync, spawn } = require("node:child_process");

const PKG_VERSION = require("./package.json").version;
const REPO = "YueMiyuki/risuko";

// -- CLI arg parsing --

const rawArgs = process.argv.slice(2);
let version = PKG_VERSION;
let noCache = false;
const appArgs = [];

for (let i = 0; i < rawArgs.length; i++) {
	const arg = rawArgs[i];
	if ((arg === "--version" || arg === "-v") && rawArgs[i + 1]) {
		version = rawArgs[++i];
	} else if (arg === "--no-cache") {
		noCache = true;
	} else if (arg === "--help" || arg === "-h") {
		console.log(`Usage: risuko-app [launcher-options] [-- app-args...]

Launcher options:
  --version <x.y.z>   Use a specific release version (default: ${PKG_VERSION})
  --no-cache          Re-download even if the binary is already cached
  -h, --help          Show this help message

Any arguments after -- are passed through to the Risuko app.

Cache location:
  macOS:   ~/Library/Application Support/risuko-launcher/<version>/
  Linux:   $XDG_DATA_HOME/risuko-launcher/<version>/
  Windows: %APPDATA%\\risuko-launcher\\<version>\\`);
		process.exit(0);
	} else if (arg === "--") {
		appArgs.push(...rawArgs.slice(i + 1));
		break;
	} else {
		appArgs.push(arg);
	}
}

// -- Platform / arch resolution --

const { platform, arch } = process;

// Asset naming scheme: Risuko_{version}_{platform}_{arch}.{ext}
// Matches process.platform and process.arch directly — no translation needed.
/** @type {Record<string, { ext: string, extract: 'tar'|'chmod'|'nsis', binary: string }>} */
const PLATFORM_INFO = {
	darwin: {
		ext: "app.tar.gz",
		extract: "tar",
		binary: path.join("Risuko.app", "Contents", "MacOS", "Risuko"),
	},
	linux: {
		ext: "AppImage",
		extract: "chmod",
		// binary is the AppImage file itself; set in getPlatformEntry()
		binary: "",
	},
	win32: {
		ext: "setup.exe",
		extract: "nsis",
		binary: "Risuko.exe",
	},
};

function getPlatformEntry() {
	const info = PLATFORM_INFO[platform];
	if (!info) {
		throw new Error(`Unsupported platform: ${platform}`);
	}
	const asset = `Risuko_${version}_${platform}_${arch}.${info.ext}`;
	const binary = platform === "linux" ? asset : info.binary;
	return { asset, extract: info.extract, binary };
}

// -- Cache directory --

function getCacheDir() {
	let base;
	if (platform === "darwin") {
		base = path.join(
			process.env.HOME || "~",
			"Library",
			"Application Support",
			"risuko-launcher",
		);
	} else if (platform === "win32") {
		base = path.join(
			process.env.APPDATA ||
				path.join(process.env.USERPROFILE || "~", "AppData", "Roaming"),
			"risuko-launcher",
		);
	} else {
		base = path.join(
			process.env.XDG_DATA_HOME ||
				path.join(process.env.HOME || "~", ".local", "share"),
			"risuko-launcher",
		);
	}
	return path.join(base, version);
}

// -- Download with redirect + progress --

/**
 * Downloads `url` to `destPath`, following redirects and printing progress.
 * @param {string} url
 * @param {string} destPath
 * @returns {Promise<void>}
 */
function download(url, destPath) {
	return new Promise((resolve, reject) => {
		const req = https.get(
			url,
			{ headers: { "User-Agent": "risuko-app-launcher" } },
			(res) => {
				if (
					res.statusCode === 301 ||
					res.statusCode === 302 ||
					res.statusCode === 307
				) {
					req.destroy();
					return download(res.headers.location, destPath).then(resolve, reject);
				}
				if (res.statusCode !== 200) {
					req.destroy();
					return reject(
						new Error(
							`Download failed: HTTP ${res.statusCode} — ${url}\n` +
								`To download manually: https://github.com/${REPO}/releases/tag/v${version}`,
						),
					);
				}

				const total = Number.parseInt(res.headers["content-length"] || "0", 10);
				let received = 0;

				fs.mkdirSync(path.dirname(destPath), { recursive: true });
				const file = fs.createWriteStream(destPath);

				res.on("data", (chunk) => {
					received += chunk.length;
					if (total > 0) {
						const pct = ((received / total) * 100).toFixed(1);
						process.stdout.write(
							`\r  Downloading… ${pct}% (${fmtBytes(received)} / ${fmtBytes(total)})`,
						);
					} else {
						process.stdout.write(`\r  Downloading… ${fmtBytes(received)}`);
					}
				});

				res.pipe(file);

				file.on("finish", () => {
					process.stdout.write("\n");
					file.close(resolve);
				});
				file.on("error", (err) => {
					fs.unlink(destPath, () => {});
					reject(err);
				});
			},
		);

		req.on("error", reject);
	});
}

function fmtBytes(n) {
	if (n >= 1024 * 1024) {
		return `${(n / 1024 / 1024).toFixed(1)} MB`;
	}
	if (n >= 1024) {
		return `${(n / 1024).toFixed(1)} KB`;
	}
	return `${n} B`;
}

// -- Extraction --

function extract(entry, assetPath, cacheDir) {
	switch (entry.extract) {
		case "tar":
			// Extracts Risuko.app/ into cacheDir
			execFileSync("tar", ["xzf", assetPath, "-C", cacheDir], {
				stdio: "inherit",
			});
			fs.unlinkSync(assetPath);
			break;

		case "chmod":
			// AppImage is the binary itself
			fs.chmodSync(assetPath, 0o755);
			break;

		case "nsis":
			// Silent NSIS install: /S = silent, /D= must be last arg (no quoting)
			execFileSync(assetPath, [`/S`, `/D=${cacheDir}`], { stdio: "inherit" });
			fs.unlinkSync(assetPath);
			break;

		default:
			throw new Error(`Unknown extract type: ${entry.extract}`);
	}
}

// -- Main --

async function main() {
	const entry = getPlatformEntry();
	const cacheDir = getCacheDir();
	const binaryPath = path.join(cacheDir, entry.binary);

	const isCached = fs.existsSync(binaryPath);

	if (!isCached || noCache) {
		const assetUrl = `https://github.com/${REPO}/releases/download/v${version}/${entry.asset}`;
		const assetPath = path.join(cacheDir, entry.asset);

		fs.mkdirSync(cacheDir, { recursive: true });

		console.log(`Downloading Risuko v${version} for ${platform}/${arch}…`);
		console.log(`  From: ${assetUrl}`);

		await download(assetUrl, assetPath);
		console.log("  Extracting…");
		extract(entry, assetPath, cacheDir);
		console.log(`  Cached to: ${cacheDir}`);
	}

	const child = spawn(binaryPath, appArgs, {
		detached: true,
		stdio: "ignore",
	});
	child.unref();
	process.exit(0);
}

main().catch((err) => {
	console.error(`\nError: ${err.message}`);
	process.exit(1);
});
