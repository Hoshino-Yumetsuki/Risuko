import assert from "node:assert/strict";
import { test } from "node:test";
import { toKebabCasePreserveNumbers } from "./configKeyCase.ts";

test("keeps canonical ED2K settings in their native kebab-case form", () => {
	const keys = ["ed2KEnableKad", "ed2KKadPort", "ed2KPort"].map(
		toKebabCasePreserveNumbers,
	);

	assert.deepEqual(keys, ["ed2k-enable-kad", "ed2k-kad-port", "ed2k-port"]);
});
