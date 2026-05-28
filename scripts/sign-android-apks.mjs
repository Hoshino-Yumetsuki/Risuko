#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

const projectRoot = resolve(new URL("..", import.meta.url).pathname);

const envFile = resolve(projectRoot, ".env");
if (existsSync(envFile)) {
	process.loadEnvFile(envFile);
}

const apkRoot = resolve(
	projectRoot,
	"src-tauri/gen/android/app/build/outputs/apk",
);

function fail(message) {
	console.error(message);
	process.exit(1);
}

function run(command, args, options = {}) {
	const result = spawnSync(command, args, {
		stdio: "inherit",
		env: process.env,
		...options,
	});
	if (result.status !== 0) {
		fail(`${command} failed with exit code ${result.status}`);
	}
}

function versionParts(version) {
	return version.split(".").map((part) => Number(part));
}

function compareVersions(a, b) {
	const left = versionParts(a);
	const right = versionParts(b);
	const length = Math.max(left.length, right.length);
	for (let index = 0; index < length; index += 1) {
		const diff = (left[index] || 0) - (right[index] || 0);
		if (diff !== 0) {
			return diff;
		}
	}
	return 0;
}

function findBuildTool(name) {
	const explicit = process.env[`${name.toUpperCase()}_PATH`];
	if (explicit) {
		return explicit;
	}
	const sdkRoot = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
	if (!sdkRoot) {
		fail("ANDROID_HOME or ANDROID_SDK_ROOT is required to locate Android build tools");
	}
	const buildToolsRoot = join(sdkRoot, "build-tools");
	if (!existsSync(buildToolsRoot)) {
		fail(`Android build-tools directory not found: ${buildToolsRoot}`);
	}
	const versions = readdirSync(buildToolsRoot)
		.filter((entry) => existsSync(join(buildToolsRoot, entry, name)))
		.sort(compareVersions)
		.reverse();
	if (versions.length === 0) {
		fail(`${name} not found under ${buildToolsRoot}`);
	}
	return join(buildToolsRoot, versions[0], name);
}

function findUnsignedApks(root) {
	if (!existsSync(root)) {
		fail(`APK output directory not found: ${root}`);
	}
	const results = [];
	const stack = [root];
	while (stack.length > 0) {
		const current = stack.pop();
		for (const entry of readdirSync(current, { withFileTypes: true })) {
			const fullPath = join(current, entry.name);
			if (entry.isDirectory()) {
				stack.push(fullPath);
			} else if (entry.isFile() && entry.name.endsWith("-unsigned.apk")) {
				results.push(fullPath);
			}
		}
	}
	return results.sort();
}

function resolveKeystore() {
	const keystorePath = process.env.ANDROID_SIGNING_KEYSTORE_PATH;
	if (keystorePath) {
		return { path: keystorePath };
	}
	const encoded = process.env.ANDROID_SIGNING_KEYSTORE_BASE64;
	if (!encoded) {
		fail(
			"ANDROID_SIGNING_KEYSTORE_PATH or ANDROID_SIGNING_KEYSTORE_BASE64 is required",
		);
	}
	const dir = mkdtempSync(join(tmpdir(), "risuko-android-signing-"));
	const path = join(dir, "release.keystore");
	writeFileSync(path, Buffer.from(encoded, "base64"));
	return { path, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

const storePassword = process.env.ANDROID_SIGNING_KEYSTORE_PASSWORD;
const alias = process.env.ANDROID_SIGNING_KEY_ALIAS;
if (!storePassword) {
	fail("ANDROID_SIGNING_KEYSTORE_PASSWORD is required");
}
if (!alias) {
	fail("ANDROID_SIGNING_KEY_ALIAS is required");
}
if (!process.env.ANDROID_SIGNING_KEY_PASSWORD) {
	process.env.ANDROID_SIGNING_KEY_PASSWORD = storePassword;
}

const apksigner = findBuildTool("apksigner");
const zipalign = findBuildTool("zipalign");
const keystore = resolveKeystore();
const unsignedApks = findUnsignedApks(apkRoot);

if (unsignedApks.length === 0) {
	fail(`No unsigned release APKs found under ${apkRoot}`);
}

try {
	for (const unsignedApk of unsignedApks) {
		const outputApk = unsignedApk.replace(/-unsigned\.apk$/, ".apk");
		const alignedApk = join(
			dirname(unsignedApk),
			`${basename(unsignedApk, ".apk")}-aligned.apk`,
		);
		run(zipalign, ["-f", "-p", "4", unsignedApk, alignedApk]);
		run(apksigner, [
			"sign",
			"--ks",
			keystore.path,
			"--ks-key-alias",
			alias,
			"--ks-pass",
			"env:ANDROID_SIGNING_KEYSTORE_PASSWORD",
			"--key-pass",
			"env:ANDROID_SIGNING_KEY_PASSWORD",
			"--out",
			outputApk,
			alignedApk,
		]);
		run(apksigner, ["verify", "--verbose", "--print-certs", outputApk]);
		rmSync(alignedApk, { force: true });
		console.log(`signed: ${outputApk}`);
	}
} finally {
	keystore.cleanup?.();
}