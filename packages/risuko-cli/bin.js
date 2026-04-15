#!/usr/bin/env node

const { execFileSync } = require("node:child_process");
const { join } = require("node:path");

const { platform, arch } = process;

const platformArchMap = {
	darwin: {
		arm64: ["@risuko/cli-darwin-arm64", "risuko"],
		x64: ["@risuko/cli-darwin-x64", "risuko"],
	},
	linux: {
		arm64: ["@risuko/cli-linux-arm64-gnu", "risuko"],
		x64: ["@risuko/cli-linux-x64-gnu", "risuko"],
	},
	win32: {
		arm64: ["@risuko/cli-win32-arm64-msvc", "risuko.exe"],
		x64: ["@risuko/cli-win32-x64-msvc", "risuko.exe"],
	},
};

function getBinaryPath() {
	const platformPackages = platformArchMap[platform];
	if (!platformPackages) {
		throw new Error(`Unsupported platform: ${platform}`);
	}

	const entry = platformPackages[arch];
	if (!entry) {
		throw new Error(`Unsupported architecture: ${platform}-${arch}`);
	}

	const [packageName, binaryName] = entry;

	try {
		const packageDir = require.resolve(`${packageName}/package.json`);
		return join(packageDir, "..", binaryName);
	} catch {
		throw new Error(
			`Failed to find binary for ${platform}-${arch}.\n` +
				`Package ${packageName} is not installed.\n` +
				`Run: npm install ${packageName}`,
		);
	}
}

try {
	const binaryPath = getBinaryPath();
	execFileSync(binaryPath, process.argv.slice(2), {
		stdio: "inherit",
		env: process.env,
	});
} catch (error) {
	if (error.status !== undefined) {
		process.exit(error.status);
	}
	console.error(error.message);
	process.exit(1);
}
