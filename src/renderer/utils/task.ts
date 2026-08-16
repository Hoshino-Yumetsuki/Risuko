import {
	ADD_TASK_TYPE,
	NONE_SELECTED_FILES,
	SELECTED_ALL_FILES,
} from "@shared/constants";
import type { AppConfig } from "@shared/types/config";
import { splitTaskLinksWithRenames } from "@shared/utils";
import {
	buildDefaultOptionsFromCurl,
	buildHeadersFromCurl,
	buildUrisFromCurl,
} from "@shared/utils/curl";
import { buildOuts } from "@shared/utils/rename";
import { isEmpty } from "lodash";
import { decodeThunderLink } from "@/utils/thunder";

interface TaskFormState {
	app: { addTaskUrl: string; addTaskOptions: Record<string, unknown> };
	preference: { config: AppConfig };
}

export interface TaskForm {
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
	completionScriptOverride: boolean;
	completionScriptCommand: string;
	completionScriptArgs: string;
	completionScriptTimeoutMs: number;
	startAt: number | null;
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
			? Math.max(1, Math.min(Math.trunc(splitNumber), 128))
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
		completionScriptOverride: false,
		completionScriptCommand: "",
		completionScriptArgs: "",
		completionScriptTimeoutMs: 30000,
		startAt: null,
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

export const buildOption = (type: string, form: TaskForm) => {
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

	if (type === ADD_TASK_TYPE.TORRENT || type === ADD_TASK_TYPE.URI) {
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
	const effectiveKey = !isEmpty(sftpPrivateKeyContent)
		? sftpPrivateKeyContent
		: sftpPrivateKey;
	if (!isEmpty(effectiveKey)) {
		result["sftp-private-key"] = effectiveKey;
	}
	if (!isEmpty(sftpKeyPassphrase)) {
		result["sftp-private-key-passphrase"] = sftpKeyPassphrase;
	}

	if (form.completionScriptOverride) {
		const command = `${form.completionScriptCommand || ""}`.trim();
		if (!isEmpty(command)) {
			result["risuko-completion-script-command"] = command;
			result["risuko-completion-script-args"] = form.completionScriptArgs || "";
			result["risuko-completion-script-timeout-ms"] =
				Number(form.completionScriptTimeoutMs) || 30000;
		} else {
			result["risuko-completion-script-enabled"] = false;
		}
	}

	if (type !== ADD_TASK_TYPE.TORRENT && form.startAt && form.startAt > 0) {
		result["risuko-start-at"] = form.startAt;
	}

	return result;
};

export const buildUriPayload = async (form: TaskForm) => {
	const { uris: rawUris, out } = form;
	if (isEmpty(rawUris)) {
		throw new Error("task.new-task-uris-required");
	}

	const parsedLines = await Promise.all(
		splitTaskLinksWithRenames(rawUris).map(async ({ uri, rename }) => ({
			uri: await decodeThunderLink(uri),
			rename,
		})),
	);
	let uriList = parsedLines.map((p) => p.uri);
	const curlHeaders = buildHeadersFromCurl(uriList);
	uriList = buildUrisFromCurl(uriList);
	const formOuts = buildOuts(uriList, out);
	const outs = uriList.map(
		(_, i) => parsedLines[i]?.rename || formOuts[i] || "",
	);

	const resolvedForm: TaskForm = buildDefaultOptionsFromCurl(form, curlHeaders);

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
