import assert from "node:assert/strict";
import { test } from "node:test";
import { decodeThunderLink } from "./thunder.ts";

const SAMPLE_THUNDER_LINK =
	"thunder://QUFtYWduZXQ6P3h0PXVybjpidGloOmE2MTRiZjQ2NDVmNTY1ODhkNzI1YzY5NTg2YjU1Y2M3MDhkMmEyZmQmYW1wO2RuPSU1QiVFNyU5NCVCNSVFNSVCRCVCMSVFNSVBNCVBOSVFNSVBMCU4Mnd3dy5keXR0ODkuY29tJTVEJUU3JThFJThCJUU4JTgwJTg1JUU1JUE0JUE5JUU0JUI4JThCNC0yMDI0X0JEJUU2JTk3JUE1JUU4JUFGJUFEJUU0JUI4JUFEJUU1JUFEJTk3Lm1wNFpa";
const EXPECTED_MAGNET_LINK =
	"magnet:?xt=urn:btih:a614bf4645f56588d725c69586b55cc708d2a2fd&dn=%5B%E7%94%B5%E5%BD%B1%E5%A4%A9%E5%A0%82www.dytt89.com%5D%E7%8E%8B%E8%80%85%E5%A4%A9%E4%B8%8B4-2024_BD%E6%97%A5%E8%AF%AD%E4%B8%AD%E5%AD%97.mp4";

test("decodes browser Thunder links without Node Buffer", () => {
	const previousBuffer = (globalThis as { Buffer?: unknown }).Buffer;
	try {
		(globalThis as { Buffer?: unknown }).Buffer = undefined;
		assert.equal(decodeThunderLink(SAMPLE_THUNDER_LINK), EXPECTED_MAGNET_LINK);
	} finally {
		(globalThis as { Buffer?: unknown }).Buffer = previousBuffer;
	}
});

test("decodes the literal issue #143 link including its closing parenthesis", () => {
	assert.equal(
		decodeThunderLink(`${SAMPLE_THUNDER_LINK})`),
		EXPECTED_MAGNET_LINK,
	);
});

test("keeps malformed Thunder links available for editing", () => {
	const malformed = "thunder://not-a-valid-payload)";
	assert.doesNotThrow(() => decodeThunderLink(malformed));
	assert.equal(decodeThunderLink(malformed), malformed);
});

test("accepts URL-safe, unpadded Thunder payloads", () => {
	const encoded = btoa(
		"AAmagnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862ZZ",
	)
		.replace(/=+$/g, "")
		.replace(/\+/g, "-")
		.replace(/\//g, "_");
	assert.equal(
		decodeThunderLink(`THUNDER://${encoded}`),
		"magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862",
	);
});
