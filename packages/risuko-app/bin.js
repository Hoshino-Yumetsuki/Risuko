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
const crypto = require("node:crypto");
const { execFileSync, spawn } = require("node:child_process");

const PKG_VERSION = require("./package.json").version;
const REPO = "YueMiyuki/risuko";
const SHA256_SIDECAR_REQUIRED_VERSION = "0.4.0";

class DownloadHttpError extends Error {
	constructor(statusCode, url) {
		super(
			`Download failed: HTTP ${statusCode} — ${url}\n` +
				`To download manually: https://github.com/${REPO}/releases/tag/v${version}`,
		);
		this.name = "DownloadHttpError";
		this.statusCode = statusCode;
		this.url = url;
	}
}

// -- CLI arg parsing --

const rawArgs = process.argv.slice(2);
let version = PKG_VERSION;
let noCache = false;
let allowLegacyNoChecksum = false;
const appArgs = [];

for (let i = 0; i < rawArgs.length; i++) {
	const arg = rawArgs[i];
	if ((arg === "--version" || arg === "-v") && rawArgs[i + 1]) {
		version = rawArgs[++i].replace(/^v/, "");
	} else if (arg === "--no-cache") {
		noCache = true;
	} else if (arg === "--allow-legacy-no-checksum") {
		allowLegacyNoChecksum = true;
	} else if (arg === "--help" || arg === "-h") {
		console.log(`Usage: risuko-app [launcher-options] [-- app-args...]

Launcher options:
  --version <x.y.z>   Use a specific release version (default: ${PKG_VERSION})
  --no-cache          Re-download even if the binary is already cached
  --allow-legacy-no-checksum
                      Allow unsigned installs when SHA-256 sidecar is missing
                      for legacy releases (pre-0.4.0 or prereleases)
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
/** @type {Record<string, { ext: string, extract: 'tar'|'chmod'|'none', binary: string }>} */
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
		ext: "portable.exe",
		extract: "none",
		// binary is the downloaded asset itself; set in getPlatformEntry()
		binary: "",
	},
};

function getPlatformEntry() {
	const info = PLATFORM_INFO[platform];
	if (!info) {
		throw new Error(`Unsupported platform: ${platform}`);
	}
	const asset = `Risuko_${version}_${platform}_${arch}.${info.ext}`;
	const binary =
		platform === "linux" || platform === "win32" ? asset : info.binary;
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
					res.statusCode === 303 ||
					res.statusCode === 307 ||
					res.statusCode === 308
				) {
					req.destroy();
					const nextUrl = new URL(res.headers.location, url).toString();
					return download(nextUrl, destPath).then(resolve, reject);
				}
				if (res.statusCode !== 200) {
					req.destroy();
					return reject(new DownloadHttpError(res.statusCode, url));
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

function downloadText(url) {
	const tmpDir = fs.mkdtempSync(
		path.join(require("node:os").tmpdir(), "risuko-launcher-"),
	);
	const tmpPath = path.join(tmpDir, "download.txt");
	return download(url, tmpPath)
		.then(() => fs.readFileSync(tmpPath, "utf8"))
		.finally(() => {
			fs.rmSync(tmpDir, { recursive: true, force: true });
		});
}

function sha256File(filePath) {
	const hash = crypto.createHash("sha256");
	const input = fs.createReadStream(filePath);
	return new Promise((resolve, reject) => {
		input.on("data", (chunk) => hash.update(chunk));
		input.on("error", reject);
		input.on("end", () => resolve(hash.digest("hex")));
	});
}

function parseExpectedSha256(text, assetName) {
	for (const line of text.split(/\r?\n/)) {
		const trimmed = line.trim();
		if (!trimmed) {
			continue;
		}
		const [hash, fileName] = trimmed.split(/\s+/, 2);
		if (
			/^[a-fA-F0-9]{64}$/.test(hash) &&
			(!fileName || fileName === assetName)
		) {
			return hash.toLowerCase();
		}
	}
	throw new Error(`No SHA-256 digest found for ${assetName}`);
}

function compareReleaseVersions(left, right) {
	const leftMatch = /^v?(\d+)\.(\d+)\.(\d+)(.*)/.exec(left);
	const rightMatch = /^v?(\d+)\.(\d+)\.(\d+)(.*)/.exec(right);
	if (!leftMatch || !rightMatch) {
		return null;
	}
	for (let i = 1; i <= 3; i++) {
		const diff = Number(leftMatch[i]) - Number(rightMatch[i]);
		if (diff !== 0) {
			return diff;
		}
	}

	function parseSuffix(suffix) {
		let prerelease = null;
		let build = null;
		const buildIdx = suffix.indexOf("+");
		if (buildIdx !== -1) {
			build = suffix.slice(buildIdx + 1);
			suffix = suffix.slice(0, buildIdx);
		}
		if (suffix === "") {
			return { prerelease, build };
		}
		if (suffix.startsWith("-")) {
			prerelease = suffix.slice(1);
		} else {
			return null;
		}
		return { prerelease, build };
	}

	const leftParsed = parseSuffix(leftMatch[4]);
	const rightParsed = parseSuffix(rightMatch[4]);
	if (!leftParsed || !rightParsed) {
		return null;
	}

	// Build metadata is ignored for precedence.
	const pre1 = leftParsed.prerelease;
	const pre2 = rightParsed.prerelease;
	if (pre1 === pre2) {
		return 0;
	}
	if (pre1 === null) {
		return 1;
	}
	if (pre2 === null) {
		return -1;
	}

	const parts1 = pre1.split(".");
	const parts2 = pre2.split(".");
	const len = Math.max(parts1.length, parts2.length);
	for (let i = 0; i < len; i++) {
		const p1 = parts1[i];
		const p2 = parts2[i];
		if (p1 === undefined) {
			return -1;
		}
		if (p2 === undefined) {
			return 1;
		}

		const isNum1 = /^[0-9]+$/.test(p1);
		const isNum2 = /^[0-9]+$/.test(p2);

		if (isNum1 && isNum2) {
			const n1 = BigInt(p1);
			const n2 = BigInt(p2);
			if (n1 !== n2) {
				return n1 < n2 ? -1 : 1;
			}
		} else if (isNum1 && !isNum2) {
			return -1;
		} else if (!isNum1 && isNum2) {
			return 1;
		} else {
			if (p1 < p2) {
				return -1;
			}
			if (p1 > p2) {
				return 1;
			}
		}
	}
	return 0;
}

function isLegacyChecksumRelease(releaseVersion) {
	const order = compareReleaseVersions(
		releaseVersion,
		SHA256_SIDECAR_REQUIRED_VERSION,
	);
	if (order === null) {
		return false;
	}
	if (order < 0) {
		return true;
	}
	// A prerelease of the exact threshold version (e.g. 0.4.0-beta.1) predates
	// the stable release that introduced checksums and must also be treated as legacy.
	if (order === 0) {
		const match = /^v?\d+\.\d+\.\d+(-[^+]*)/.exec(releaseVersion);
		return match !== null;
	}
	return false;
}

async function verifySha256(assetPath, checksumUrl, assetName) {
	let checksumText;
	try {
		checksumText = await downloadText(checksumUrl);
	} catch (err) {
		if (err instanceof DownloadHttpError && err.statusCode === 404) {
			if (isLegacyChecksumRelease(version) && allowLegacyNoChecksum) {
				return false;
			}
			throw new Error(
				`SHA-256 sidecar not found for ${assetName}: ${checksumUrl}`,
			);
		}
		throw err;
	}
	const expected = parseExpectedSha256(checksumText, assetName);
	const actual = await sha256File(assetPath);
	if (actual !== expected) {
		throw new Error(
			`SHA-256 mismatch for ${assetName}: expected ${expected}, got ${actual}`,
		);
	}
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

		case "none":
			// Portable Windows exe — already in place, no extraction needed.
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
		const checksumUrl = `${assetUrl}.sha256`;
		const assetPath = path.join(cacheDir, entry.asset);
		const tmpPath = `${assetPath}.download`;

		fs.mkdirSync(cacheDir, { recursive: true });

		console.log(`Downloading Risuko v${version} for ${platform}/${arch}…`);
		console.log(`  From: ${assetUrl}`);

		try {
			await download(assetUrl, tmpPath);
			const checksumVerified = await verifySha256(
				tmpPath,
				checksumUrl,
				entry.asset,
			);
			if (checksumVerified === false) {
				console.warn(
					`  SHA-256 sidecar not found for legacy release v${version}; continuing without checksum verification`,
				);
			} else {
				console.log("  Verified SHA-256 checksum");
			}
			fs.renameSync(tmpPath, assetPath);
		} catch (err) {
			fs.rmSync(tmpPath, { force: true });
			throw err;
		}
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
