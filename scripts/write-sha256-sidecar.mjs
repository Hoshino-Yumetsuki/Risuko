#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createReadStream, writeFileSync } from "node:fs";
import { basename } from "node:path";

const assetPath = process.argv[2] ?? process.env.ASSET_PATH;

if (!assetPath) {
	console.error("Usage: node scripts/write-sha256-sidecar.mjs <asset-path>");
	process.exit(2);
}

const digest = await new Promise((resolve, reject) => {
	const hash = createHash("sha256");
	const stream = createReadStream(assetPath);

	stream.on("data", (chunk) => hash.update(chunk));
	stream.on("error", reject);
	stream.on("end", () => resolve(hash.digest("hex")));
});

writeFileSync(`${assetPath}.sha256`, `${digest}  ${basename(assetPath)}\n`);
