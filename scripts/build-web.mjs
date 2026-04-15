/**
 * Build the web UI and copy it into packages/risuko-app/dist/
 *
 * Usage: node scripts/build-web.mjs
 */
import { execSync } from "node:child_process";
import { cpSync, mkdirSync, rmSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const webDist = resolve(root, "dist/web");
const pkgDist = resolve(root, "packages/risuko-app/dist");

console.log("Building web UI with Vite...\n");
execSync("npx vite build --config vite.web.config.ts", {
	cwd: root,
	stdio: "inherit",
});

if (!existsSync(webDist)) {
	console.error("Build output not found at dist/web/");
	process.exit(1);
}

// Copy to package dist
if (existsSync(pkgDist)) {
	rmSync(pkgDist, { recursive: true });
}
mkdirSync(pkgDist, { recursive: true });
cpSync(webDist, pkgDist, { recursive: true });

console.log(`\nWeb UI copied to packages/risuko-app/dist/`);
