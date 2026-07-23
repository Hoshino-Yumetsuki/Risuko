import { systemKeys, userKeys } from "@shared/configKeys";
import {
	APP_THEME,
	ENGINE_RPC_HOST,
	SUPPORT_RTL_LOCALES,
	TASK_STATUS,
	TEMP_DOWNLOAD_SUFFIX,
} from "@shared/constants";
import { getLanguage } from "@shared/locales";
import type { DownloadTask } from "@shared/types/task";
import {
	camelCase,
	compact,
	isArray,
	isPlainObject,
	mapKeys,
	omitBy,
	pick,
} from "lodash";

export { bytesToSize } from "./format";

// human message from an axios-style error, with a 429 special-case
// and a caller-supplied fallback; shared by the sync + share stores
export const getApiErrorMessage = (
	err: unknown,
	fallback = "Request failed",
): string => {
	const response = (
		err as { response?: { data?: { error?: string }; status?: number } }
	).response;
	if (response?.data?.error) {
		return response.data.error;
	}
	if (response?.status === 429) {
		return "Too many requests. Please try again later.";
	}
	return (err as Error)?.message || fallback;
};

export const extractSpeedUnit = (speed = "") => {
	if (parseInt(speed, 10) === 0) {
		return "K";
	}

	const regex = /^(\d+\.?\d*)([KMG])$/;
	const match = regex.exec(speed);

	if (!match) {
		return "K";
	}

	return match[2];
};

export const parseBooleanConfig = (value: unknown, fallback = false) => {
	if (typeof value === "boolean") {
		return value;
	}

	if (typeof value === "number") {
		return value !== 0;
	}

	if (typeof value === "string") {
		const normalized = value.trim().toLowerCase();
		if (["true", "1", "yes", "on"].includes(normalized)) {
			return true;
		}
		if (["false", "0", "no", "off", ""].includes(normalized)) {
			return false;
		}
	}

	return fallback;
};

export const calcProgress = (totalLength, completedLength, decimal = 2) => {
	const total = parseInt(totalLength, 10);
	const completed = parseInt(completedLength, 10);
	if (total === 0 || completed === 0) {
		return 0;
	}
	const percentage = (completed / total) * 100;
	const result = parseFloat(percentage.toFixed(decimal));
	return result;
};

export const calcRatio = (totalLength, uploadLength) => {
	const total = parseInt(totalLength, 10);
	const upload = parseInt(uploadLength, 10);
	if (total === 0 || upload === 0) {
		return 0;
	}

	const percentage = upload / total;
	const result = parseFloat(percentage.toFixed(4));
	return result;
};

export const timeRemaining = (totalLength, completedLength, downloadSpeed) => {
	const remainingLength = totalLength - completedLength;
	if (remainingLength <= 0 || downloadSpeed <= 0) {
		return 0;
	}
	return Math.ceil(remainingLength / downloadSpeed);
};

/**
 * timeFormat
 * @param {int} seconds
 * @param {string} prefix
 * @param {string} suffix
 * @param {object} i18n
 * i18n: {
 *  gt1d: 'More than one day',
 *  hour: 'h',
 *  minute: 'm',
 *  second: 's'
 * }
 */
export const timeFormat = (seconds, { prefix = "", suffix = "", i18n }) => {
	let result = "";
	let hours = "";
	let minutes = "";
	let secs = seconds || 0;
	const i = {
		gt1d: "> 1 day",
		hour: "h",
		minute: "m",
		second: "s",
		...i18n,
	};

	if (secs <= 0) {
		return "";
	}
	if (secs > 86400) {
		return `${prefix} ${i.gt1d} ${suffix}`;
	}
	if (secs > 3600) {
		hours = `${Math.floor(secs / 3600)}${i.hour} `;
		secs %= 3600;
	}
	if (secs > 60) {
		minutes = `${Math.floor(secs / 60)}${i.minute} `;
		secs %= 60;
	}
	secs += i.second;
	result = hours + minutes + secs;
	return result ? `${prefix} ${result} ${suffix}` : result;
};

export const localeDateTimeFormat = (timestamp, locale) => {
	if (!timestamp) {
		return "";
	}

	if (`${timestamp}`.length === 10) {
		timestamp *= 1000;
	}
	const date = new Date(timestamp);
	return date.toLocaleDateString(getLanguage(locale || "en-US"), {
		year: "numeric",
		month: "long",
		day: "numeric",
		hour: "numeric",
		minute: "numeric",
		second: "numeric",
	});
};

const ellipsis = (str = "", maxLen = 64) => {
	if (str.length < maxLen) {
		return str;
	}
	return maxLen > 0 ? `${str.substring(0, maxLen)}...` : str;
};

export const stripTempDownloadSuffix = (name = "") => {
	const value = `${name || ""}`;
	if (!value.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX)) {
		return value;
	}
	return value.slice(0, value.length - TEMP_DOWNLOAD_SUFFIX.length);
};

export const getTaskName = (task, options = {}) => {
	const o = {
		defaultName: "",
		maxLen: 64, // -1: no length limit
		...options,
	};
	const { defaultName, maxLen } = o;
	let result = defaultName;
	if (!task) {
		return result;
	}

	const files = Array.isArray(task.files) ? task.files : [];
	const { bittorrent } = task;

	if (bittorrent?.info?.name) {
		result = ellipsis(bittorrent.info.name, maxLen);
	} else if (files.length > 0) {
		result = getFileNameFromFile(files[0]);
		if (task.status === TASK_STATUS.COMPLETE) {
			result = stripTempDownloadSuffix(result);
		}
		result = ellipsis(result, maxLen);
	}

	return result;
};

export const getFileNameFromFile = (file) => {
	if (!file) {
		return "";
	}

	let { path } = file;
	if (!path && file.uris && file.uris.length > 0) {
		path = decodeURI(file.uris[0].uri);
	}

	const index = path.lastIndexOf("/");

	if (index < 0 || index === path.length - 1) {
		return path;
	}

	return path.substring(index + 1);
};

export const isMagnetTask = (task) => {
	const { bittorrent } = task;
	return bittorrent && !bittorrent.info;
};

const hasUriScheme = (input: string, schemes: string[]) => {
	if (!input || typeof input !== "string") {
		return false;
	}
	const lower = input.trim().toLowerCase();
	return schemes.some((s) => lower.startsWith(`${s}:`));
};

const isAdcUri = (uri: string) =>
	hasUriScheme(uri, ["adc", "adcs", "dchub", "nmdc"]);
const isGnutellaUri = (uri: string) => hasUriScheme(uri, ["gnutella", "gnet"]);
const isG2Uri = (uri: string) => hasUriScheme(uri, ["g2"]);
const isGiftUri = (uri: string) => hasUriScheme(uri, ["gift"]);
const isMagnetUri = (uri: string) => hasUriScheme(uri, ["magnet"]);
const isEd2kUri = (uri: string) => hasUriScheme(uri, ["ed2k"]);
const isThunderUri = (uri: string) =>
	hasUriScheme(uri, ["thunder", "flashget", "qqdl"]);

const YOUTUBE_HOSTS = new Set([
	"youtu.be",
	"youtube.com",
	"youtube-nocookie.com",
]);

// registrable-domain suffixes routed to the yt-dlp media engine; keep in
// sync with MEDIA_HOST_SUFFIXES in
// src-tauri/risuko-engine/src/engine/media.rs
const MEDIA_HOST_SUFFIXES = [
	"youtube.com",
	"youtu.be",
	"youtube-nocookie.com",
	"twitch.tv",
	"vimeo.com",
	"dailymotion.com",
	"dai.ly",
	"bilibili.com",
	"b23.tv",
	"nicovideo.jp",
	"tiktok.com",
	"twitter.com",
	"x.com",
	"instagram.com",
	"facebook.com",
	"fb.watch",
	"reddit.com",
	"redd.it",
	"soundcloud.com",
	"bandcamp.com",
	"streamable.com",
	"rumble.com",
	"odysee.com",
	"bitchute.com",
	"ted.com",
	"vk.com",
];

const hostnameOf = (uri: string): string | null => {
	const trimmed = uri.trim();
	if (!trimmed) {
		return null;
	}
	const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)
		? trimmed
		: `https://${trimmed}`;
	try {
		return new URL(withScheme).hostname.toLowerCase();
	} catch {
		return null;
	}
};

const isYoutubeUri = (uri: string): boolean => {
	if (!uri || typeof uri !== "string") {
		return false;
	}
	const hostname = hostnameOf(uri);
	if (!hostname) {
		return false;
	}
	if (YOUTUBE_HOSTS.has(hostname)) {
		return true;
	}
	return (
		hostname.endsWith(".youtube.com") ||
		hostname.endsWith(".youtube-nocookie.com")
	);
};

// true when the URL's host is in the media allowlist; mirrors the engine's
// is_media_uri. URLs outside the list can still be forced via the add-task
// "Download with yt-dlp" toggle
export const isMediaUri = (uri: string): boolean => {
	if (!uri || typeof uri !== "string") {
		return false;
	}
	// enforce HTTP(S) protocol to match engine behavior
	if (!uri.startsWith("http://") && !uri.startsWith("https://")) {
		return false;
	}
	const hostname = hostnameOf(uri);
	if (!hostname) {
		return false;
	}
	return MEDIA_HOST_SUFFIXES.some(
		(suffix) => hostname === suffix || hostname.endsWith(`.${suffix}`),
	);
};

const isM3u8Uri = (uri: string): boolean => {
	if (!uri || typeof uri !== "string") {
		return false;
	}
	const lower = uri.trim().toLowerCase().split("#")[0].split("?")[0];
	return lower.endsWith(".m3u8") || lower.endsWith(".m3u");
};

/**
 * Friendly protocol label inferred from a URI, or `null` when the scheme is
 * unrecognised. Order matters: specific schemes (magnet, yt-dlp hosts, m3u8
 * path suffix) take precedence over generic HTTP
 */
export const detectUriProtocol = (uri: string): string | null => {
	if (!uri || typeof uri !== "string") {
		return null;
	}
	const trimmed = uri.trim();
	if (!trimmed) {
		return null;
	}
	if (isMagnetUri(trimmed)) {
		return "Magnet";
	}
	if (isEd2kUri(trimmed)) {
		return "ED2K";
	}
	if (isAdcUri(trimmed)) {
		return "ADC / Direct Connect";
	}
	if (isGnutellaUri(trimmed)) {
		return "Gnutella";
	}
	if (isG2Uri(trimmed)) {
		return "Gnutella2 (G2)";
	}
	if (isGiftUri(trimmed)) {
		return "giFT";
	}
	if (isThunderUri(trimmed)) {
		return "Thunder";
	}
	if (isSftpLink(trimmed)) {
		return "SFTP";
	}
	if (isFtpsLink(trimmed)) {
		return "FTPS";
	}
	if (isFtpLink(trimmed)) {
		return "FTP";
	}
	if (isYoutubeUri(trimmed)) {
		return "YouTube";
	}
	if (isMediaUri(trimmed)) {
		return "Media (yt-dlp)";
	}
	if (isM3u8Uri(trimmed)) {
		return "M3U8 / HLS";
	}
	const lower = trimmed.toLowerCase();
	if (lower.startsWith("https://")) {
		return "HTTPS";
	}
	if (lower.startsWith("http://")) {
		return "HTTP";
	}
	return null;
};

export const checkTaskIsSeeder = (task) => {
	const { bittorrent, seeder } = task;
	return !!bittorrent && seeder === "true";
};

export const getTaskUri = (task, withTracker = false) => {
	const { files } = task;
	let result = "";
	if (checkTaskIsBT(task)) {
		result = buildMagnetLink(task, withTracker);
		return result;
	}

	if (files && files.length === 1) {
		const { uris } = files[0];
		result = uris[0].uri;
	}

	return result;
};

const buildMagnetLink = (task, withTracker = false) => {
	const { bittorrent, infoHash } = task;
	const { info } = bittorrent;

	const params = [`magnet:?xt=urn:btih:${infoHash}`];
	if (info?.name) {
		params.push(`dn=${encodeURI(info.name)}`);
	}

	if (withTracker) {
		(bittorrent.announceList || []).forEach((tracker) => {
			params.push(`tr=${encodeURI(tracker)}`);
		});
	}

	return params.join("&");
};

// guard, not default param: a failed tell_status nulls the current task
// and default params only cover undefined
export const checkTaskIsBT = (task?: Partial<DownloadTask> | null) =>
	!!task?.bittorrent;

// Task kinds (engine-side `TaskKind`, lowercase) where force-pause + resume
// recovers a stalled connection: a single TCP socket gets stuck and
// reopening it kicks the download free. HTTP/HTTPS, FTP/SFTP (both folded
// into `ftp`), HTTP-segment streams (`media`, `m3u8`) all fit
//
// Peer-swarm protocols (`torrent`, `ed2k`, `adc`, `gnutella`, `g2`, `gift`)
// do NOT. Pause tears down peer connections and aborts in-flight tracker
// announces and choke-unchoke negotiations; a swarm that spent minutes
// warming up is wiped in 500 ms, and the recovery cooldown refires before
// it can rebuild, leaving speed permanently at zero
const LOW_SPEED_RECOVERABLE_KINDS = new Set(["http", "ftp", "media", "m3u8"]);

export const taskBenefitsFromLowSpeedRecovery = (
	task: Partial<DownloadTask> = {},
) => {
	if (checkTaskIsBT(task)) {
		return false;
	}
	// legacy/unknown task shapes that lack `kind` but expose a peer-swarm
	// sentinel field must still be excluded
	if (task.ed2kLink) {
		return false;
	}
	const kind = `${task.kind || ""}`.toLowerCase();
	if (!kind) {
		// no kind reported. Without a positive signal we can't tell whether
		// pause/resume is safe, so opt out — losing recovery on an HTTP task
		// is harmless, applying it to a swarm task is destructive
		return false;
	}
	return LOW_SPEED_RECOVERABLE_KINDS.has(kind);
};

const toKebabCasePreserveNumbers = (key = "") => {
	return `${key}`
		.replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
		.replace(/([a-z0-9])([A-Z])/g, "$1-$2")
		.replace(/[_\s]+/g, "-")
		.toLowerCase();
};

export const changeKeysToCamelCase = (obj = {}) => {
	return mapKeys(obj, (_v, k) => camelCase(k));
};

export const changeKeysToKebabCase = (obj = {}) => {
	return mapKeys(obj, (_v, k) => toKebabCasePreserveNumbers(k));
};

export const separateConfig = (options) => {
	const user = {};
	const system = {};
	const others = {};

	for (const [k, v] of Object.entries(options)) {
		if (userKeys.includes(k)) {
			user[k] = v;
		} else if (systemKeys.includes(k)) {
			system[k] = v;
		} else {
			others[k] = v;
		}
	}
	return { user, system, others };
};

const splitTextRows = (text = "") => {
	text = `${text}`;
	let result =
		text
			.replace(/(?:\\\r\\\n|\\\r|\\\n)/g, " ")
			.replace(/(?:\r\n|\r|\n)/g, "\n")
			.split("\n") || [];
	result = result.map((row) => row.trim());
	return result;
};

export const convertCommaToLine = (text = "") => {
	return `${text}`
		.split(",")
		.map((row) => row.trim())
		.join("\n")
		.trim();
};

export const convertLineToComma = (text = "") => {
	return text.trim().replace(/(?:\r\n|\r|\n)/g, ",");
};

export const filterFilesBySuffix = (files = [], suffixes = []) => {
	return files.filter((item) => suffixes.includes(item.extension));
};

const decodeThunderLink = (url = "") => {
	if (!url.startsWith("thunder://")) {
		return url;
	}

	let result = url.trim();
	result = result.split("thunder://")[1];
	result = Buffer.from(result, "base64").toString("utf8");
	result = result.substring(2, result.length - 2);
	return result;
};

// trailing ` Rename: <filename>` on a link line overrides the output
// filename per URL without the form `out` field. Capture is greedy to
// end-of-line so filenames with spaces survive; we trim and reject path
// separators to keep it strictly a name
const RENAME_SUFFIX_REGEX = /\s+Rename:\s*(\S.*?)\s*$/i;

export interface ParsedTaskLink {
	uri: string;
	rename?: string;
}

const parseRenameDirective = (line = ""): ParsedTaskLink => {
	const m = RENAME_SUFFIX_REGEX.exec(line);
	if (!m) {
		return { uri: line };
	}
	const rename = m[1].trim();
	const uri = line.slice(0, m.index).trim();
	// reject path separators — `out` is a filename, never a path
	if (!rename || rename.includes("/") || rename.includes("\\")) {
		return { uri };
	}
	return { uri, rename };
};

export const splitTaskLinks = (links = "") => {
	return splitTaskLinksWithRenames(links).map((t) => t.uri);
};

export const splitTaskLinksWithRenames = (links = ""): ParsedTaskLink[] => {
	return compact(splitTextRows(links))
		.map(parseRenameDirective)
		.map(({ uri, rename }) => ({ uri: decodeThunderLink(uri), rename }));
};

const isFtpLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("ftp://");
};

const isFtpsLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("ftps://");
};

const isSftpLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("sftp://");
};

export const getLangDirection = (locale = "en-US") => {
	return SUPPORT_RTL_LOCALES.includes(locale) ? "rtl" : "ltr";
};

export const listTorrentFiles = (files) => {
	const result = files.map((file, index) => {
		const extension = getFileExtension(file.path);
		const item = {
			// aria2 select-file start index at 1
			// possible values: 1-1048576
			idx: index + 1,
			extension: `.${extension}`,
			...file,
		};
		return item;
	});
	return result;
};

export const getFileName = (fullPath) => {
	return fullPath.replace(/^.*[\\/]/, "");
};

export const getFileExtension = (filename) => {
	return filename.slice(((filename.lastIndexOf(".") - 1) >>> 0) + 2);
};

export const removeExtensionDot = (extension = "") => {
	return extension.replace(".", "");
};

export const diffConfig = (current = {}, next = {}) => {
	const curr = pick(current, Object.keys(next));
	const result = omitBy(next, (val, key) => {
		if (isArray(val) || isPlainObject(val)) {
			return JSON.stringify(curr[key]) === JSON.stringify(val);
		}
		return curr[key] === val;
	});

	return result;
};

export const parseHeader = (header = "") => {
	header = header.trim();
	let result: Record<string, string> = {};
	if (!header) {
		return result;
	}

	const headers = splitTextRows(header);
	headers.forEach((line) => {
		const index = line.indexOf(":");
		const name = line.substring(0, index);
		const value = line.substring(index + 1).trim();
		result[name] = value;
	});
	result = changeKeysToCamelCase(result) as Record<string, string>;

	return result;
};

export const formatOptionsForEngine = (
	options: Record<string, unknown> = {},
) => {
	const result: Record<string, string> = {};

	Object.keys(options).forEach((key) => {
		const kebabCaseKey = toKebabCasePreserveNumbers(key);
		if (Array.isArray(options[key])) {
			result[kebabCaseKey] = options[key].join("\n");
		} else {
			result[kebabCaseKey] = `${options[key]}`;
		}
	});

	return result;
};

export const buildRpcUrl = (
	options: { host?: string; port?: number | string; secret?: string } = {},
) => {
	const { host = ENGINE_RPC_HOST, port, secret } = options;
	let result = `${host}:${port}/jsonrpc`;
	if (secret) {
		result = `token:${secret}@${result}`;
	}
	result = `http://${result}`;

	return result;
};

export const generateRandomInt = (min = 0, max = 10000) => {
	return min + Math.floor(Math.random() * (max - min));
};

export const pushItemToFixedLengthArray = (arr = [], maxLength, item) => {
	const result =
		arr.length >= maxLength
			? [...arr.slice(1, maxLength), item]
			: [...arr, item];
	return result;
};

export const removeArrayItem = (arr = [], item) => {
	const idx = arr.indexOf(item);
	if (idx === -1) {
		return [...arr];
	}

	const result = [...arr.slice(0, idx), ...arr.slice(idx + 1)];
	return result;
};

export const getInverseTheme = (theme) => {
	return theme === APP_THEME.LIGHT ? APP_THEME.DARK : APP_THEME.LIGHT;
};

export const changedConfig = { basic: {}, advanced: {} };
