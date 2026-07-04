#!/usr/bin/env node


import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export function buildManifest(dir, { version, repo } = {}) {
	version = String(version ?? "").replace(/^v/i, "");
	if (!version) throw new Error("version required");
	if (!repo) throw new Error("repo (owner/repo) required");
	const base = `https://github.com/${repo}/releases/download/v${version}`;
	const platforms = {};
	for (const file of readdirSync(dir)) {
		if (!file.endsWith(".json")) continue;
		const frag = JSON.parse(readFileSync(join(dir, file), "utf8"));
		if (!frag.key || !frag.asset || !frag.signature) {
			throw new Error(`incomplete fragment: ${file}`);
		}
		platforms[frag.key] = { signature: frag.signature, url: `${base}/${frag.asset}` };
	}
	if (Object.keys(platforms).length === 0) throw new Error(`no fragments in ${dir}`);

	return { version, notes: "", pub_date: new Date().toISOString(), platforms };
}

// CLI: node scripts/build-updater-manifest.mjs <fragments-dir> [out]
if (import.meta.url === `file://${process.argv[1]}`) {
	const [dir, out = "latest.json"] = process.argv.slice(2);
	if (!dir) {
		console.error("Usage: build-updater-manifest.mjs <fragments-dir> [out]");
		process.exit(2);
	}
	const manifest = buildManifest(dir, {
		version: process.env.VERSION,
		repo: process.env.REPO,
	});
	writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
	console.log(`wrote ${out}: ${Object.keys(manifest.platforms).join(", ")}`);
}
