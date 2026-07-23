import assert from "node:assert/strict";
import { test } from "node:test";
import { bytesToSize } from "./format.ts";

test("treats negative byte counts as zero", () => {
	assert.equal(bytesToSize(-1), "0 KB");
});
