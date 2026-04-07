import { systemKeys, userKeys } from "@shared/configKeys";
import {
	APP_THEME,
	AUDIO_SUFFIXES,
	DOCUMENT_SUFFIXES,
	ENGINE_RPC_HOST,
	IMAGE_SUFFIXES,
	RESOURCE_TAGS,
	SUB_SUFFIXES,
	SUPPORT_RTL_LOCALES,
	TASK_STATUS,
	TEMP_DOWNLOAD_SUFFIX,
	UNKNOWN_PEERID,
	UNKNOWN_PEERID_NAME,
	VIDEO_SUFFIXES,
} from "@shared/constants";
import type { DownloadTask } from "@shared/types/task";
import {
	camelCase,
	compact,
	difference,
	isArray,
	isEmpty,
	isFunction,
	isPlainObject,
	omitBy,
	pick,
} from "lodash";

export const bytesToSize = (bytes, precision = 1) => {
	const b = parseInt(bytes, 10);
	const sizes = ["B", "KB", "MB", "GB", "TB"];
	if (b === 0) {
		return "0 KB";
	}
	const i = Math.floor(Math.log(b) / Math.log(1024));
	if (i === 0) {
		return `${b} ${sizes[i]}`;
	}
	return `${(b / 1024 ** i).toFixed(precision)} ${sizes[i]}`;
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

export const bitfieldToPercent = (text) => {
	const len = text.length - 1;
	let p: number;
	let one = 0;
	for (let i = 0; i < len; i++) {
		p = parseInt(text[i], 16);
		for (let j = 0; j < 4; j++) {
			one += p & 1;
			p >>= 1;
		}
	}
	return Math.floor((one / (4 * len)) * 100).toString();
};

export const peerIdParser = (str) => {
	if (!str || str === UNKNOWN_PEERID) {
		return UNKNOWN_PEERID_NAME;
	}

	// With the native engine, peer client info is provided directly.
	// Return the string as-is if it looks like a client name, otherwise return unknown.
	if (typeof str === "string" && str.length > 0) {
		return str;
	}

	return UNKNOWN_PEERID_NAME;
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
	return date.toLocaleDateString(locale, {
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

const stripTempDownloadSuffix = (name = "") => {
	const value = `${name || ""}`;
	if (!value.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX)) {
		return value;
	}
	return value.slice(0, value.length - TEMP_DOWNLOAD_SUFFIX.length);
};

export const getTaskName = (task, options = {}) => {
	const o = {
		defaultName: "",
		maxLen: 64, // -1: No limit length
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

	if (index <= 0 || index === path.length) {
		return path;
	}

	return path.substring(index + 1);
};

export const isMagnetTask = (task) => {
	const { bittorrent } = task;
	return bittorrent && !bittorrent.info;
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

const buildMagnetLink = (task, withTracker = false, btTracker = []) => {
	const { bittorrent, infoHash } = task;
	const { info } = bittorrent;

	const params = [`magnet:?xt=urn:btih:${infoHash}`];
	if (info?.name) {
		params.push(`dn=${encodeURI(info.name)}`);
	}

	if (withTracker) {
		const trackers = difference(bittorrent.announceList, btTracker);
		trackers.forEach((tracker) => {
			params.push(`tr=${encodeURI(tracker)}`);
		});
	}

	return params.join("&");
};

export const checkTaskIsBT = (task: Partial<DownloadTask> = {}) => {
	const { bittorrent } = task;
	return !!bittorrent;
};

const changeKeysCase = (obj, caseConverter) => {
	const result = {};
	if (isEmpty(obj) || !isFunction(caseConverter)) {
		return result;
	}

	for (const [k, value] of Object.entries(obj)) {
		const key = caseConverter(k);
		result[key] = value;
	}

	return result;
};

const toKebabCasePreserveNumbers = (key = "") => {
	return `${key}`
		.replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
		.replace(/([a-z0-9])([A-Z])/g, "$1-$2")
		.replace(/[_\s]+/g, "-")
		.toLowerCase();
};

export const changeKeysToCamelCase = (obj = {}) => {
	return changeKeysCase(obj, camelCase);
};

export const changeKeysToKebabCase = (obj = {}) => {
	return changeKeysCase(obj, toKebabCasePreserveNumbers);
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

export const filterVideoFiles = (files = []) => {
	const suffix = [...VIDEO_SUFFIXES, ...SUB_SUFFIXES];
	return files.filter((item) => {
		return suffix.includes(item.extension);
	});
};

export const filterAudioFiles = (files = []) => {
	return files.filter((item) => {
		return AUDIO_SUFFIXES.includes(item.extension);
	});
};

export const filterImageFiles = (files = []) => {
	return files.filter((item) => {
		return IMAGE_SUFFIXES.includes(item.extension);
	});
};

export const filterDocumentFiles = (files = []) => {
	return files.filter((item) => {
		return DOCUMENT_SUFFIXES.includes(item.extension);
	});
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

export const splitTaskLinks = (links = "") => {
	return compact(splitTextRows(links)).map(decodeThunderLink);
};

interface Ed2kFileLink {
	fileName: string;
	fileSize: number;
	fileHash: string;
	sources: Array<{ ip: string; port: number }>;
	aichHash: string;
}

export const isEd2kLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("ed2k://");
};

/**
 * Parse an ed2k file link into structured data.
 * Format: ed2k://|file|<name>|<size>|<hash>|/  (with optional |h=<AICH>| and |sources,...|)
 */
export const parseEd2kLink = (url: string): Ed2kFileLink | null => {
	const trimmed = (url || "").trim();
	if (!isEd2kLink(trimmed)) {
		return null;
	}

	// Remove "ed2k://|file|" prefix and trailing "|/"
	const body = trimmed
		.replace(/^ed2k:\/\/\|file\|/i, "")
		.replace(/\|\/\s*$/, "");
	if (!body) {
		return null;
	}

	const parts = body.split("|");
	if (parts.length < 3) {
		return null;
	}

	const fileName = decodeURIComponent(parts[0] || "").replace(/_/g, " ");
	const fileSize = parseInt(parts[1], 10);
	const fileHash = (parts[2] || "").toLowerCase();

	if (!fileName || !Number.isFinite(fileSize) || fileSize <= 0) {
		return null;
	}

	if (!/^[0-9a-f]{32}$/.test(fileHash)) {
		return null;
	}

	const sources: Array<{ ip: string; port: number }> = [];
	let aichHash = "";

	// Parse optional trailing segments
	for (let i = 3; i < parts.length; i++) {
		const seg = parts[i];
		if (seg.startsWith("h=")) {
			aichHash = seg.slice(2);
		} else if (seg.startsWith("sources,")) {
			const sourceEntries = seg.slice("sources,".length).split(",");
			for (const entry of sourceEntries) {
				const [ip, portStr] = entry.split(":");
				const port = parseInt(portStr, 10);
				if (ip && Number.isFinite(port) && port > 0 && port <= 65535) {
					sources.push({ ip, port });
				}
			}
		}
	}

	return { fileName, fileSize, fileHash, sources, aichHash };
};

export const isM3u8Link = (uri: string): boolean => {
	const trimmed = uri.trim().toLowerCase();
	const path = trimmed.split("?")[0].split("#")[0];
	return path.endsWith(".m3u8") || path.endsWith(".m3u");
};

const isFtpLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("ftp://");
};

const isFtpsLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("ftps://");
};

export const isSftpLink = (uri: string): boolean => {
	return uri.trim().toLowerCase().startsWith("sftp://");
};

export const isFtpFamily = (uri: string): boolean => {
	return isFtpLink(uri) || isFtpsLink(uri) || isSftpLink(uri);
};

export const detectResource = (content) => {
	return RESOURCE_TAGS.some((type) => {
		return content.includes(type);
	});
};

export const getLangDirection = (locale = "en-US") => {
	return SUPPORT_RTL_LOCALES.includes(locale) ? "rtl" : "ltr";
};

export const listTorrentFiles = (files) => {
	const result = files.map((file, index) => {
		const extension = getFileExtension(file.path);
		const item = {
			// aria2 select-file start index at 1
			// possible Values: 1-1048576
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

export const calcFormLabelWidth = (locale = "en-US") => {
	return typeof locale === "string" && locale.startsWith("de") ? "28%" : "25%";
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

export const intersection = (array1 = [], array2 = []) => {
	if (array1.length === 0 || array2.length === 0) {
		return [];
	}

	const set = new Set(array2);
	return array1.filter((value) => set.has(value));
};

export const cloneArray = (arr = [], reversed = false) => {
	if (!Array.isArray(arr)) {
		return arr;
	}

	const result = [...arr];
	return reversed ? result.reverse() : result;
};

export const pushItemToFixedLengthArray = (arr = [], maxLength, item) => {
	const result =
		arr.length >= maxLength
			? [...arr.slice(1, maxLength - 1), item]
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
