#!/usr/bin/env node

import { existsSync, globSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const packageDir = process.cwd();
const packagePath = join(packageDir, "package.json");
const pkg = JSON.parse(readFileSync(packagePath, "utf8"));
const files = Array.isArray(pkg.files) ? pkg.files : [];
const missing = [];

function isGlobPattern(rel) {
	return /[*?[\]{}]/.test(rel);
}

for (const rel of files) {
	const matches = isGlobPattern(rel)
		? globSync(rel, { cwd: packageDir })
		: [rel];

	if (matches.length === 0) {
		missing.push(`${rel} (missing)`);
		continue;
	}

	for (const match of matches) {
		const path = resolve(packageDir, match);
		if (!existsSync(path)) {
			missing.push(`${match} (missing)`);
			continue;
		}
		if (statSync(path).size === 0) {
			missing.push(`${match} (empty)`);
		}
	}
}

if (missing.length > 0) {
	console.error(
		[
			`Package ${pkg.name} is missing required artifact(s):`,
			...missing.map((entry) => `  - ${entry}`),
			"Run the release staging step before packing or publishing this platform package.",
		].join("\n"),
	);
	process.exit(1);
}
