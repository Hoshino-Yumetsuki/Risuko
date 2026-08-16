import {
	MAX_BT_TRACKER_LENGTH,
	ONE_SECOND,
	PROXY_SCOPES,
} from "@shared/constants";
import axios from "axios";

const TRACKER_SOURCE_CACHE_TTL = 10 * 60 * ONE_SECOND;
const trackerSourceCache = new Map<
	string,
	{ expiresAt: number; value: string[] }
>();
const trackerSourceInFlight = new Map<string, Promise<string[]>>();

const normalizeSource = (source: string[] = []) => {
	return [
		...new Set(source.map((item) => String(item).trim()).filter(Boolean)),
	].sort();
};

const convertToAxiosProxy = (proxyServer = "") => {
	if (!proxyServer) {
		return;
	}

	const { username, password, protocol, hostname, port } = new URL(proxyServer);
	return {
		protocol: protocol.replace(":", ""),
		host: hostname,
		port: port ? Number(port) : undefined,
		...(username || password ? { auth: { username, password } } : {}),
	};
};

const buildTrackerSourceCacheKey = (source = [], proxyServer = "") => {
	return JSON.stringify({
		proxyServer,
		source: normalizeSource(source),
	});
};

const getCachedTrackerSource = (key: string, now: number) => {
	const cached = trackerSourceCache.get(key);
	if (!cached) {
		return;
	}

	if (cached.expiresAt > now) {
		return cached.value;
	}

	trackerSourceCache.delete(key);
};

export const fetchBtTrackerFromSource = async (
	source: string[],
	proxyConfig: {
		enable?: boolean;
		server?: string;
		bypass?: string;
		scope?: string[];
	} = {},
	resolveProxy?: (url: string) => Promise<string | null>,
	fetchSource?: (urls: string[]) => Promise<string[]>,
) => {
	if (!source?.length) {
		return [];
	}

	const now = Date.now();
	const { enable, server, scope = [] } = proxyConfig;
	const proxyEnabled =
		enable && server && scope.includes(PROXY_SCOPES.UPDATE_TRACKERS);
	const proxyCacheKey = fetchSource
		? `${enable ? "1" : "0"}|${server || ""}|${proxyConfig.bypass || ""}|${scope.join(",")}`
		: proxyEnabled
			? `${server}|${proxyConfig.bypass || ""}`
			: "";
	const cacheKey = buildTrackerSourceCacheKey(source, proxyCacheKey);
	const cached = getCachedTrackerSource(cacheKey, now);
	if (cached) {
		return cached;
	}

	const inFlight = trackerSourceInFlight.get(cacheKey);
	if (inFlight) {
		return inFlight;
	}

	const requestPromise = (async () => {
		const requestUrls = source.map((url) => appendCacheBust(url, now));
		if (fetchSource) {
			const values = await fetchSource(requestUrls);
			const result = [...new Set(values)];
			trackerSourceCache.set(cacheKey, {
				value: result,
				expiresAt: Date.now() + TRACKER_SOURCE_CACHE_TTL,
			});
			return result;
		}
		const proxies = proxyEnabled
			? await Promise.all(
					requestUrls.map(async (requestUrl) => {
						const resolved = resolveProxy
							? await resolveProxy(requestUrl)
							: server || null;
						return resolved ? convertToAxiosProxy(resolved) : undefined;
					}),
				)
			: requestUrls.map(() => undefined);

		const results = await Promise.allSettled(
			requestUrls.map((requestUrl, index) =>
				axios
					.get(requestUrl, {
						timeout: 30 * ONE_SECOND,
						proxy: proxies[index],
					})
					.then((value) => value.data),
			),
		);
		const values = results
			.filter(
				(item): item is PromiseFulfilledResult<string> =>
					item.status === "fulfilled",
			)
			.map((item) => item.value);
		const result = [...new Set(values)];
		trackerSourceCache.set(cacheKey, {
			value: result,
			expiresAt: Date.now() + TRACKER_SOURCE_CACHE_TTL,
		});
		return result;
	})().finally(() => {
		trackerSourceInFlight.delete(cacheKey);
	});

	trackerSourceInFlight.set(cacheKey, requestPromise);
	return requestPromise;
};

function appendCacheBust(source: string, now: number): string {
	try {
		const url = new URL(source);
		url.searchParams.set("t", `${now}`);
		return url.toString();
	} catch {
		const separator = source.includes("?") ? "&" : "?";
		return `${source}${separator}t=${now}`;
	}
}

export const convertTrackerDataToLine = (arr = []) => {
	return arr
		.join("\r\n")
		.replace(/^\s*[\r\n]/gm, "")
		.trim();
};

export const reduceTrackerString = (str = "") => {
	if (str.length <= MAX_BT_TRACKER_LENGTH) {
		return str;
	}

	const subStr = str.substring(0, MAX_BT_TRACKER_LENGTH);
	const index = subStr.lastIndexOf(",");
	if (index === -1) {
		return subStr;
	}

	const result = subStr.substring(0, index);
	return result;
};
