#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

const assetPath = process.argv[2] ?? process.env.ASSET_PATH;

if (!assetPath) {
	console.error("Usage: node scripts/write-sha256-sidecar.mjs <asset-path>");
	process.exit(2);
}

const digest = createHash("sha256")
	.update(readFileSync(assetPath))
	.digest("hex");

writeFileSync(`${assetPath}.sha256`, `${digest}  ${basename(assetPath)}\n`);
