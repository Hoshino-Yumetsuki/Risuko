import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildManifest } from "./build-updater-manifest.mjs";

const dir = mkdtempSync(join(tmpdir(), "frag-"));
writeFileSync(
	join(dir, "darwin-aarch64.json"),
	JSON.stringify({
		key: "darwin-aarch64",
		asset: "Risuko_1.2.3_darwin_arm64.app.tar.gz",
		signature: "SIG_A",
	}),
);
writeFileSync(
	join(dir, "windows-x86_64.json"),
	JSON.stringify({
		key: "windows-x86_64",
		asset: "Risuko_1.2.3_win32_x64.setup.exe",
		signature: "SIG_W",
	}),
);

const m = buildManifest(dir, {
	version: "v1.2.3",
	repo: "YueMiyuki/Risuko",
});

assert.equal(m.version, "1.2.3");
assert.match(m.pub_date, /^\d{4}-\d{2}-\d{2}T/);
assert.equal(m.platforms["darwin-aarch64"].signature, "SIG_A");
assert.equal(
	m.platforms["darwin-aarch64"].url,
	"https://github.com/YueMiyuki/Risuko/releases/download/v1.2.3/Risuko_1.2.3_darwin_arm64.app.tar.gz",
);
assert.equal(
	m.platforms["windows-x86_64"].url,
	"https://github.com/YueMiyuki/Risuko/releases/download/v1.2.3/Risuko_1.2.3_win32_x64.setup.exe",
);
assert.throws(() => buildManifest(dir, { repo: "x/y" }), /version required/);

rmSync(dir, { recursive: true, force: true });
console.log("ok");
