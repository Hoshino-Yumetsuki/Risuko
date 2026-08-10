import assert from "node:assert/strict";
import { test } from "node:test";
import {
	findHttpSourceUrl,
	getErrorCodeReferenceUrl,
	isHttpUrl,
	openExternalUrl,
} from "./external.ts";

test("accepts only browser-safe HTTP(S) URLs", () => {
	assert.equal(isHttpUrl(" https://example.com/download "), true);
	assert.equal(isHttpUrl("http://example.com:8080/file"), true);
	assert.equal(isHttpUrl("https://"), false);
	assert.equal(isHttpUrl("file:///tmp/file"), false);
	assert.equal(isHttpUrl("javascript:alert(1)"), false);
	assert.equal(isHttpUrl("magnet:?xt=urn:btih:abc"), false);
	assert.equal(isHttpUrl(""), false);
});

test("finds a valid persisted source URL without exposing other URI schemes", () => {
	assert.equal(
		findHttpSourceUrl({
			files: [{ uris: [{ uri: "magnet:?xt=urn:btih:abc" }] }],
			"source-url": "https://example.com/original",
		}),
		"https://example.com/original",
	);

	assert.equal(
		findHttpSourceUrl({
			options: { "original-url": "http://cdn.example.com/archive.zip" },
		}),
		"http://cdn.example.com/archive.zip",
	);
	assert.equal(findHttpSourceUrl({ uri: "file:///tmp/archive.zip" }), "");
});

test("builds error-code reference URLs only for canonical numeric codes", () => {
	assert.equal(
		getErrorCodeReferenceUrl("315"),
		"https://risuko.app/docs/reference/error-codes#315",
	);
	assert.equal(
		getErrorCodeReferenceUrl(540),
		"https://risuko.app/docs/reference/error-codes#540",
	);
	assert.equal(getErrorCodeReferenceUrl("0"), "");
	assert.equal(getErrorCodeReferenceUrl("315#other"), "");
	assert.equal(getErrorCodeReferenceUrl("not-a-code"), "");
});

test("does not invoke an external opener for invalid URLs", async () => {
	assert.equal(await openExternalUrl("javascript:alert(1)"), false);
});

test("treats a browser fallback with noopener as a dispatched open", async () => {
	const originalWindow = globalThis.window;
	let openArgs: [string, string | undefined, string | undefined] | undefined;
	Object.defineProperty(globalThis, "window", {
		configurable: true,
		value: {
			open: (url: string, target?: string, features?: string) => {
				openArgs = [url, target, features];
				return null;
			},
		},
	});

	try {
		assert.equal(await openExternalUrl("https://example.com"), true);
		assert.deepEqual(openArgs, [
			"https://example.com",
			"_blank",
			"noopener,noreferrer",
		]);
	} finally {
		Object.defineProperty(globalThis, "window", {
			configurable: true,
			value: originalWindow,
		});
	}
});
