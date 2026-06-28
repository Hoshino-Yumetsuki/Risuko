#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectRoot = resolve(scriptDir, "..");
const gradleDir = join(projectRoot, "src-tauri/gen/android");
const gradlePropsPath = join(gradleDir, "gradle.properties");
const tuningPath = join(scriptDir, "android-gradle.properties");
const marker = "# Risuko Gradle tuning (scripts/patch-android-gradle.mjs)";
const staleTuningKeys = [
	"org.gradle.jvmargs=-Xmx4g",
	"org.gradle.parallel=true",
	"org.gradle.configuration-cache=true",
	"org.gradle.caching=true",
];

export function patchAndroidGradle() {
	if (!existsSync(gradleDir)) {
		return false;
	}
	const tuning = readFileSync(tuningPath, "utf8").trim();
	let content = existsSync(gradlePropsPath)
		? readFileSync(gradlePropsPath, "utf8")
		: "";
	const markerIndex = content.indexOf(marker);
	if (markerIndex >= 0) {
		content = content.slice(0, markerIndex);
	}
	content = content
		.split("\n")
		.filter((line) => !staleTuningKeys.some((key) => line.trim().startsWith(key)))
		.join("\n")
		.trimEnd();
	const next = `${content}\n\n${marker}\n${tuning}\n`;
	writeFileSync(gradlePropsPath, next);
	return true;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
	const patched = patchAndroidGradle();
	if (!patched) {
		console.warn(
			"[Risuko] Android project not initialized yet; gradle tuning will apply on first build",
		);
	}
}
