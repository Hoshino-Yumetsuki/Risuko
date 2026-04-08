import {
	ADD_TASK_TYPE,
	NONE_SELECTED_FILES,
	SELECTED_ALL_FILES,
} from "@shared/constants";
import type { AppConfig } from "@shared/types/config";
import type { SavedCredential } from "@shared/types/credential";
import { isFtpFamily, isSftpLink, splitTaskLinks } from "@shared/utils";
import {
	buildDefaultOptionsFromCurl,
	buildHeadersFromCurl,
	buildUrisFromCurl,
} from "@shared/utils/curl";
import { buildOuts } from "@shared/utils/rename";
import { isEmpty } from "lodash";
import api from "@/api";

interface TaskFormState {
	app: { addTaskUrl: string; addTaskOptions: Record<string, unknown> };
	preference: { config: AppConfig };
}

interface TaskForm {
	allProxy: string;
	cookie: string;
	dir: string;
	fileCategoryDirs: Record<string, string>;
	followTorrent: boolean | undefined;
	newTaskShowDownloading: boolean | undefined;
	out: string;
	referer: string;
	selectFile: string;
	split: number;
	torrentPath: string;
	uris: string;
	userAgent: string;
	authorization: string;
	ftpUser: string;
	ftpPasswd: string;
	sftpPrivateKey: string;
	sftpPrivateKeyContent: string;
	sftpKeyPassphrase: string;
	[key: string]: unknown;
}

export const initTaskForm = (state: TaskFormState) => {
	const { addTaskUrl, addTaskOptions } = state.app;
	const {
		allProxy,
		cookie,
		dir,
		fileCategoryDirs,
		followTorrent,
		newTaskShowDownloading,
		referer,
		split,
		userAgent,
	} = state.preference.config;
	const splitNumber = Number(split);
	const normalizedSplit =
		Number.isFinite(splitNumber) && splitNumber > 0
			? Math.max(1, Math.min(Math.trunc(splitNumber), 16))
			: 16;

	const result = {
		allProxy,
		cookie: cookie || "",
		dir,
		fileCategoryDirs: fileCategoryDirs || {},
		followTorrent,
		newTaskShowDownloading,
		out: "",
		referer: referer || "",
		selectFile: NONE_SELECTED_FILES,
		split: normalizedSplit,
		torrentPath: "",
		uris: addTaskUrl,
		userAgent: userAgent || "",
		authorization: "",
		ftpUser: state.preference.config["ftp-user"] || "",
		ftpPasswd: state.preference.config["ftp-passwd"] || "",
		sftpPrivateKey: state.preference.config["sftp-private-key"] || "",
		sftpPrivateKeyContent: "",
		sftpKeyPassphrase:
			state.preference.config["sftp-private-key-passphrase"] || "",
		...addTaskOptions,
	};
	return result;
};

const buildHeader = (form: TaskForm) => {
	const { cookie, authorization } = form;
	const result = [];
	if (!isEmpty(cookie)) {
		result.push(`Cookie: ${cookie}`);
	}
	if (!isEmpty(authorization)) {
		result.push(`Authorization: ${authorization}`);
	}

	return result;
};

const buildOption = (type: string, form: TaskForm) => {
	const { allProxy, dir, out, referer, selectFile, split, userAgent } = form;
	const result: Record<string, unknown> = {};

	if (!isEmpty(allProxy)) {
		result.allProxy = allProxy;
	}

	if (!isEmpty(dir)) {
		result.dir = dir;
	}

	if (!isEmpty(out)) {
		result.out = out;
	}

	if (split > 0) {
		result.split = split;
	}

	if (!isEmpty(userAgent)) {
		result.userAgent = userAgent;
	}

	if (!isEmpty(referer)) {
		result.referer = referer;
	}

	if (type === ADD_TASK_TYPE.TORRENT) {
		const normalizedSelectFile = `${selectFile || ""}`.trim();
		const hasExplicitSelection =
			normalizedSelectFile &&
			normalizedSelectFile !== SELECTED_ALL_FILES &&
			normalizedSelectFile !== NONE_SELECTED_FILES;
		if (hasExplicitSelection) {
			result.selectFile = normalizedSelectFile;
		}
	}

	const header = buildHeader(form);
	if (!isEmpty(header)) {
		result.header = header;
	}

	// FTP / SFTP credentials
	const {
		ftpUser,
		ftpPasswd,
		sftpPrivateKey,
		sftpPrivateKeyContent,
		sftpKeyPassphrase,
	} = form;
	if (!isEmpty(ftpUser)) {
		result["ftp-user"] = ftpUser;
	}
	if (!isEmpty(ftpPasswd)) {
		result["ftp-passwd"] = ftpPasswd;
	}
	// Inline key content takes precedence over file path
	const effectiveKey = !isEmpty(sftpPrivateKeyContent)
		? sftpPrivateKeyContent
		: sftpPrivateKey;
	if (!isEmpty(effectiveKey)) {
		result["sftp-private-key"] = effectiveKey;
	}
	if (!isEmpty(sftpKeyPassphrase)) {
		result["sftp-private-key-passphrase"] = sftpKeyPassphrase;
	}

	return result;
};

export const buildUriPayload = async (form: TaskForm) => {
	const { uris: rawUris, out } = form;
	if (isEmpty(rawUris)) {
		throw new Error("task.new-task-uris-required");
	}

	let uriList = splitTaskLinks(rawUris);
	const curlHeaders = buildHeadersFromCurl(uriList);
	uriList = buildUrisFromCurl(uriList);
	const outs = buildOuts(uriList, out);

	let resolvedForm: TaskForm = buildDefaultOptionsFromCurl(form, curlHeaders);

	// Apply category-based directory via backend resolution
	const fileCategoryDirs = resolvedForm.fileCategoryDirs || {};
	if (uriList.length > 0) {
		const firstUri = uriList[0];
		const outName = outs[0] || (await inferOutFromUri(firstUri));
		const category = await api.resolveFileCategory(outName || firstUri);
		if (category && fileCategoryDirs[category]) {
			resolvedForm = { ...resolvedForm, dir: fileCategoryDirs[category] };
		}
	}

	const options = buildOption(ADD_TASK_TYPE.URI, resolvedForm);
	const result = {
		uris: uriList,
		outs,
		options,
	};
	return result;
};

export const buildTorrentPayload = (form: TaskForm) => {
	const { torrentPath } = form;
	if (isEmpty(torrentPath)) {
		throw new Error("task.new-task-torrent-required");
	}

	const options = buildOption(ADD_TASK_TYPE.TORRENT, form);
	const result = {
		torrentPath,
		options,
	};
	return result;
};

/**
 * Infer output filename from a URI via the Rust backend.
 */
export async function inferOutFromUri(uri: string): Promise<string> {
	const raw = (uri || "").trim();
	if (!raw) {
		return "";
	}
	try {
		return await api.inferOutFromUri(raw);
	} catch {
		return "";
	}
}

const AUTH_FIELDS: (keyof SavedCredential)[] = [
	"authorization",
	"cookie",
	"ftpUser",
	"ftpPasswd",
	"sftpPrivateKey",
	"sftpPrivateKeyContent",
	"sftpKeyPassphrase",
	"allProxy",
];

export function extractHostFromUri(uri: string): string | null {
	const raw = (uri || "").trim();
	if (!raw) {
		return null;
	}
	const firstLine = raw.split("\n")[0].trim();
	try {
		const url = new URL(firstLine);
		return url.hostname || null;
	} catch {
		return null;
	}
}

export function extractProtocolFromUri(uri: string): string | undefined {
	const raw = (uri || "").trim();
	if (!raw) {
		return undefined;
	}
	const firstLine = raw.split("\n")[0].trim().toLowerCase();
	if (isSftpLink(firstLine)) {
		return "sftp";
	}
	if (isFtpFamily(firstLine)) {
		return "ftp";
	}
	return "http";
}

export function extractCredentialFromForm(
	form: TaskForm,
): Partial<SavedCredential> {
	const result: Record<string, string> = {};
	for (const key of AUTH_FIELDS) {
		const val = form[key];
		if (typeof val === "string" && val.trim()) {
			result[key] = val;
		}
	}
	return result as Partial<SavedCredential>;
}

export function applyCredentialToForm(
	form: TaskForm,
	credential: SavedCredential,
): void {
	for (const key of AUTH_FIELDS) {
		const val = credential[key];
		if (typeof val === "string" && val.trim()) {
			form[key] = val;
		}
	}
}

export function credentialHasContent(
	credential: Partial<SavedCredential>,
): boolean {
	return AUTH_FIELDS.some((key) => {
		const val = credential[key];
		return typeof val === "string" && val.trim().length > 0;
	});
}
