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
