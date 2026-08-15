import assert from "node:assert/strict";
import test from "node:test";

import { validateFeatureInterpolations } from "./check-i18n.mjs";

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
