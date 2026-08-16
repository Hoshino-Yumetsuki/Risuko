const THUNDER_SCHEME = /^thunder:\/\//i;
const TRAILING_NON_BASE64_CHARS = /[^A-Za-z0-9+/_=-]+$/;

const decodeBase64Utf8 = (value: string): string => {
	const normalized = value
		.replace(/\s+/g, "")
		.replace(/-/g, "+")
		.replace(/_/g, "/");
	if (!normalized || !/^[A-Za-z0-9+/]*={0,2}$/.test(normalized)) {
		throw new Error("Invalid Thunder payload");
	}

	const padding = (4 - (normalized.length % 4)) % 4;
	const binary = atob(`${normalized}${"=".repeat(padding)}`);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i += 1) {
		bytes[i] = binary.charCodeAt(i);
	}
	return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
};

const decodeHtmlAmpersands = (value: string): string =>
	value.replace(/&(?:amp|#0*38|#x0*26);/gi, "&");

export const decodeThunderLink = (url = ""): string => {
	const original = url;
	const trimmed = url.trim();
	if (!THUNDER_SCHEME.test(trimmed)) {
		return url;
	}

	try {
		const encoded = trimmed
			.slice(trimmed.indexOf("://") + 3)
			.replace(TRAILING_NON_BASE64_CHARS, "");
		const decoded = decodeBase64Utf8(encoded);
		if (!decoded.startsWith("AA") || !decoded.endsWith("ZZ")) {
			return original;
		}

		const uri = decoded.slice(2, -2).trim();
		return uri ? decodeHtmlAmpersands(uri) : original;
	} catch {
		return original;
	}
};
