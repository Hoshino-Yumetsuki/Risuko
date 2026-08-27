import assert from "node:assert/strict";
import { test } from "node:test";
import { buildMagnetLink } from "./magnetLink.ts";

test("encodes tracker query delimiters as a single magnet parameter value", () => {
	const tracker = "https://tracker.example/announce?pass=a&user=b#frag";
	const uri = buildMagnetLink(
		{
			bittorrent: {
				info: { name: "file" },
				announceList: [[tracker]],
			},
			infoHash: "abc",
		},
		true,
	);

	assert.equal(uri.includes(`tr=${encodeURIComponent(tracker)}`), true, uri);
	assert.equal(
		uri.includes("tr=https://tracker.example/announce?pass=a&user=b"),
		false,
	);
});

test("encodes display-name reserved characters as a single magnet parameter", () => {
	const name = "a&b?c#d";
	const uri = buildMagnetLink({
		bittorrent: {
			info: { name },
			announceList: [],
		},
		infoHash: "abc",
	});

	assert.equal(uri, `magnet:?xt=urn:btih:abc&dn=${encodeURIComponent(name)}`);
	assert.equal(uri.includes("&b"), false, uri);
	assert.equal(uri.includes("?c"), false, uri);
	assert.equal(uri.includes("#d"), false, uri);
});
