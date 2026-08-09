import { invoke } from "@tauri-apps/api/core";

export function isHttpUrl(value: unknown): value is string {
	if (typeof value !== "string" || !value.trim()) {
		return false;
	}
	try {
		const url = new URL(value.trim());
		return (
			(url.protocol === "http:" || url.protocol === "https:") &&
			url.hostname.length > 0
		);
	} catch {
		return false;
	}
}

export function findHttpSourceUrl(value: unknown): string {
	if (!value || typeof value !== "object") {
		return "";
	}

	const task = value as {
		files?: Array<{ uris?: Array<{ uri?: unknown }> }>;
		m3u8Link?: unknown;
		"m3u8-link"?: unknown;
		sourceUrl?: unknown;
		"source-url"?: unknown;
		webpageUrl?: unknown;
		"webpage-url"?: unknown;
		originalUrl?: unknown;
		"original-url"?: unknown;
		uri?: unknown;
		url?: unknown;
		options?: Record<string, unknown>;
	};
	const candidates: unknown[] = [];
	for (const file of Array.isArray(task.files) ? task.files : []) {
		for (const source of Array.isArray(file?.uris) ? file.uris : []) {
			candidates.push(source?.uri);
		}
	}
	candidates.push(
		task.m3u8Link,
		task["m3u8-link"],
		task.sourceUrl,
		task["source-url"],
		task.webpageUrl,
		task["webpage-url"],
		task.originalUrl,
		task["original-url"],
		task.uri,
		task.url,
		task.options?.sourceUrl,
		task.options?.["source-url"],
		task.options?.url,
		task.options?.webpageUrl,
		task.options?.["webpage-url"],
		task.options?.originalUrl,
		task.options?.["original-url"],
	);

	return candidates.find((candidate) => isHttpUrl(candidate))?.trim() || "";
}

export async function openExternalUrl(value: unknown): Promise<boolean> {
	if (!isHttpUrl(value)) {
		return false;
	}
	const url = value.trim();
	try {
		await invoke("plugin:shell|open", { path: url });
	} catch {
		// Browser fallback keeps the action usable in web preview/dev builds.
		// `openExternalUrl` is also used by store/toast code that can be
		// exercised during SSR or unit tests, where `window` does not exist.
		const browser = (
			globalThis as typeof globalThis & {
				window?: {
					open?: (url: string, target?: string, features?: string) => unknown;
				};
			}
		).window;
		if (typeof browser?.open !== "function") {
			return false;
		}
		return Boolean(browser.open(url, "_blank", "noopener,noreferrer"));
	}
	return true;
}
