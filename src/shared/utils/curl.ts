import curlParser from "@bany/curl-to-json";

export const buildUrisFromCurl = (uris = []) => {
	return uris.map((uri) => {
		if (uri.startsWith("curl")) {
			const parsedUri = curlParser(uri);
			uri = parsedUri.url;
			if (parsedUri.params && Object.keys(parsedUri.params).length > 0) {
				const paramsStr = Object.keys(parsedUri.params)
					.map((k) => `${k}=${parsedUri.params[k]}`)
					.join("&");
				uri = `${uri}?${paramsStr}`;
			}
			return uri;
		} else {
			return uri;
		}
	});
};

export const buildHeadersFromCurl = (uris = []) => {
	return uris.map((uri) => {
		if (uri.startsWith("curl")) {
			const parsed = curlParser(uri) as unknown as Record<string, unknown>;
			const header: Record<string, string> = {
				...((parsed.header as Record<string, string>) ?? {}),
			};
			const cookie = parsed.cookie as string | undefined;
			const userAgent = parsed["user-agent"] as string | undefined;
			const referer = parsed.referer as string | undefined;
			if (cookie) {
				header.cookie = cookie;
			}
			if (userAgent) {
				header["user-agent"] = userAgent;
			}
			if (referer) {
				header.referer = referer;
			}
			return header;
		} else {
			return undefined;
		}
	});
};

export const buildDefaultOptionsFromCurl = (form, headers = []) => {
	const firstNonNullHeader = headers.find((elem) => elem);
	if (firstNonNullHeader) {
		if (firstNonNullHeader.cookie !== undefined) {
			form.cookie = firstNonNullHeader.cookie;
		}
		if (firstNonNullHeader.referer !== undefined) {
			form.referer = firstNonNullHeader.referer;
		}
		if (firstNonNullHeader["user-agent"] !== undefined) {
			form.userAgent = firstNonNullHeader["user-agent"];
		}
		if (firstNonNullHeader.authorization !== undefined) {
			form.authorization = firstNonNullHeader.authorization;
		}
	}
	return form;
};
