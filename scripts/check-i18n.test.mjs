import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { isDirectExecution, validateFeatureInterpolations } from "./check-i18n.mjs";

const interpolationKey = "preferences.engine-overrides-reserved-keys";

test("required interpolation rejects a non-literal initializer", () => {
	const errors = validateFeatureInterpolations(
		"en-US",
		new Set([interpolationKey]),
		new Map(),
	);

	assert.deepEqual(errors, [
		"en-US: preferences.engine-overrides-reserved-keys must be a string literal containing {{keys}}",
	]);
});

test("required interpolation accepts the i18next placeholder", () => {
	const errors = validateFeatureInterpolations(
		"en-US",
		new Set([interpolationKey]),
		new Map([[interpolationKey, "Reserved keys: {{keys}}"]]),
	);

	assert.deepEqual(errors, []);
});

test("checker runs when invoked through a symlinked scripts directory", () => {
	const temporaryDirectory = mkdtempSync(join(tmpdir(), "risuko-check-i18n-"));
	try {
		const scriptsDirectory = dirname(fileURLToPath(import.meta.url));
		const linkedScriptsDirectory = join(temporaryDirectory, "scripts");
		symlinkSync(
			scriptsDirectory,
			linkedScriptsDirectory,
			process.platform === "win32" ? "junction" : "dir",
		);

		const result = spawnSync(
			process.execPath,
			[join(linkedScriptsDirectory, "check-i18n.mjs")],
			{ encoding: "utf8" },
		);

		assert.equal(result.status, 0, result.stderr);
		assert.match(result.stdout, /^i18n parity OK:/m);
	} finally {
		rmSync(temporaryDirectory, { recursive: true, force: true });
	}
});

test("direct-execution detection fails loudly for an unresolvable entry", () => {
	const missingEntry = join(tmpdir(), `missing-check-i18n-${process.pid}.mjs`);

	assert.throws(
		() => isDirectExecution(missingEntry),
		/Unable to resolve i18n checker entry path/,
	);
});
