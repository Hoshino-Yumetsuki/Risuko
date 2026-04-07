import {
	APP_THEME,
	TASK_STATUS,
	TEMP_DOWNLOAD_SUFFIX,
} from "@shared/constants";
import type { DownloadTask } from "@shared/types/task";
import { getFileNameFromFile, isMagnetTask } from "@shared/utils";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "vue-sonner";

function joinPath(...parts: string[]): string {
	const joined = parts.filter(Boolean).join("/");
	return joined.replace(/[/\\]+/g, "/");
}

const hasTempDownloadSuffix = (fullPath = ""): boolean => {
	return `${fullPath || ""}`.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX);
};

const stripTempDownloadSuffix = (fullPath = ""): string => {
	const value = `${fullPath || ""}`;
	if (!hasTempDownloadSuffix(value)) {
		return value;
	}
	return value.slice(0, value.length - TEMP_DOWNLOAD_SUFFIX.length);
};

export const showItemInFolder = async (
	fullPath: string,
	{ errorMsg, fallbackPath }: { errorMsg?: string; fallbackPath?: string } = {},
) => {
	const revealPath = `${fullPath || ""}`.trim();
	const fallback = `${fallbackPath || ""}`.trim();
	if (!revealPath && !fallback) {
		return;
	}

	logger.info(
		`[Motrix] showItemInFolder: path="${revealPath}", fallback="${fallback}"`,
	);

	try {
		await invoke("reveal_in_folder", { path: revealPath || fallback });
	} catch (err) {
		logger.warn(`[Motrix] showItemInFolder fail: ${err}`);

		if (fallback && fallback !== revealPath) {
			try {
				await invoke("reveal_in_folder", { path: fallback });
				return;
			} catch (fallbackErr) {
				logger.warn(`[Motrix] showItemInFolder fallback fail: ${fallbackErr}`);
			}
		}

		if (errorMsg) {
			toast.error(errorMsg);
		}
	}
};

export const openItem = async (fullPath: string) => {
	if (!fullPath) {
		return;
	}
	return invoke("open_path", { path: fullPath });
};

export const getTaskFullPath = (
	task: DownloadTask,
	options: { normalizeCompletedPath?: boolean } = {},
): string => {
	const { normalizeCompletedPath = true } = options;
	const { dir, files, bittorrent } = task;
	let result = dir;

	if (isMagnetTask(task)) {
		return result;
	}

	const isBtMultiFile =
		!!bittorrent?.info?.name && Array.isArray(files) && files.length > 1;
	if (isBtMultiFile) {
		return joinPath(result, bittorrent.info.name);
	}

	const file = Array.isArray(files) && files.length > 0 ? files[0] : undefined;
	const path = file?.path || "";

	if (path) {
		result = path;
	} else if (files?.length === 1) {
		const fileName = getFileNameFromFile(file);
		if (fileName) {
			result = joinPath(result, fileName);
		}
	}

	if (normalizeCompletedPath && task?.status === TASK_STATUS.COMPLETE) {
		return stripTempDownloadSuffix(result);
	}

	return result;
};

export const getTaskRevealPath = (task: DownloadTask): string => {
	if (!task) {
		return "";
	}

	if (isMagnetTask(task)) {
		const result = `${task?.dir || ""}`.trim();
		logger.info(`[Motrix] getTaskRevealPath (magnet): "${result}"`);
		return result;
	}

	const files = Array.isArray(task?.files) ? task.files : [];
	const candidate =
		`${files.find((file) => `${file?.path || ""}`.trim())?.path || ""}`.trim();
	if (!candidate) {
		const fallback = getTaskFullPath(task);
		logger.info(
			`[Motrix] getTaskRevealPath (no file.path, using getTaskFullPath): "${fallback}"`,
		);
		return fallback;
	}

	const result =
		task?.status === TASK_STATUS.COMPLETE
			? stripTempDownloadSuffix(candidate)
			: candidate;
	logger.info(`[Motrix] getTaskRevealPath (from file.path): "${result}"`);
	return result;
};

export const finalizeCompletedDownloadPath = async (
	task: DownloadTask,
): Promise<string> => {
	if (!task) {
		return "";
	}

	if (isMagnetTask(task)) {
		return getTaskFullPath(task);
	}

	const sourcePath = getTaskFullPath(task, {
		normalizeCompletedPath: false,
	});
	if (!hasTempDownloadSuffix(sourcePath)) {
		return sourcePath;
	}

	const targetPath = stripTempDownloadSuffix(sourcePath);
	if (!targetPath || targetPath === sourcePath) {
		return sourcePath;
	}

	try {
		await invoke("rename_path", {
			fromPath: sourcePath,
			toPath: targetPath,
		});
		return targetPath;
	} catch (err) {
		logger.warn(`[Motrix] rename completed temp file failed: ${err}`);
		return sourcePath;
	}
};

export const moveTaskFilesToTrash = async (
	task: DownloadTask,
): Promise<boolean> => {
	const { dir, status, bittorrent } = task;
	const files = Array.isArray(task.files) ? task.files : [];

	if (isMagnetTask(task)) {
		// Magnet task with no metadata resolved — try trashing the dir as last resort
		// (but don't trash the top-level download dir itself)
		await cleanupGeneratedTorrentSidecars(task);
		return true;
	}

	logger.info(
		`[Motrix] moveTaskFilesToTrash: dir="${dir}", status="${status}", files=${files.length}`,
	);

	// For multi-file BT tasks, trash the torrent folder
	const isBtMultiFile = !!bittorrent?.info?.name && files.length > 1;
	if (isBtMultiFile) {
		const torrentFolder = joinPath(dir, bittorrent.info.name);
		logger.info(`[Motrix] trashing torrent folder: "${torrentFolder}"`);
		try {
			const found: boolean = await invoke("trash_item", {
				path: torrentFolder,
			});
			if (found) {
				logger.info(`[Motrix] trashed torrent folder: "${torrentFolder}"`);
			}
		} catch (err) {
			logger.warn(`[Motrix] trash torrent folder failed: ${err}`);
			// Fall through to try individual files
			let trashedAny = false;
			for (const file of files) {
				const filePath = `${file?.path || ""}`.trim();
				if (!filePath || filePath === dir) {
					continue;
				}
				try {
					const found: boolean = await invoke("trash_item", { path: filePath });
					if (found) {
						trashedAny = true;
					}
				} catch (fileErr) {
					logger.warn(`[Motrix] trash file "${filePath}" failed: ${fileErr}`);
				}
			}
			if (!trashedAny) {
				await cleanupGeneratedTorrentSidecars(task);
				return false;
			}
		}
		await cleanupGeneratedTorrentSidecars(task);
		return true;
	}

	// Single file task (HTTP or single-file BT)
	const path = getTaskFullPath(task);
	logger.info(
		`[Motrix] moveTaskFilesToTrash: path="${path}", dir="${dir}", status="${status}"`,
	);

	if (!path || dir === path) {
		throw new Error("task.file-path-error");
	}

	// For incomplete tasks, the file on disk may still have the .part suffix
	const partPath =
		status !== TASK_STATUS.COMPLETE &&
		!path.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX)
			? `${path}${TEMP_DOWNLOAD_SUFFIX}`
			: null;

	try {
		const found: boolean = await invoke("trash_item", { path });
		if (found) {
			logger.info(`[Motrix] trashed: "${path}"`);
		} else if (partPath) {
			const partFound: boolean = await invoke("trash_item", { path: partPath });
			if (partFound) {
				logger.info(`[Motrix] trashed .part file: "${partPath}"`);
			}
		}
	} catch (err) {
		logger.warn(`[Motrix] trash "${path}" failed: ${err}`);
		if (partPath) {
			try {
				const partFound: boolean = await invoke("trash_item", {
					path: partPath,
				});
				if (partFound) {
					logger.info(`[Motrix] trashed .part file: "${partPath}"`);
					await cleanupGeneratedTorrentSidecars(task);
					return true;
				}
			} catch (partErr) {
				logger.warn(
					`[Motrix] trash .part "${partPath}" also failed: ${partErr}`,
				);
			}
		}
		await cleanupGeneratedTorrentSidecars(task);
		return false;
	}

	await cleanupGeneratedTorrentSidecars(task);

	return true;
};

const cleanupGeneratedTorrentSidecars = async (
	task: DownloadTask,
): Promise<number> => {
	try {
		const result = await invoke<number>(
			"cleanup_generated_torrent_sidecars_for_task",
			{
				task,
			},
		);
		return Number.isFinite(result) ? result : 0;
	} catch (err) {
		logger.warn(`[Motrix] cleanup generated torrent sidecars failed: ${err}`);
		return 0;
	}
};

export const getSystemTheme = (): string => {
	if (window.matchMedia?.("(prefers-color-scheme: dark)").matches) {
		return APP_THEME.DARK;
	}
	return APP_THEME.LIGHT;
};

export const protectDownloadFile = async (
	task: DownloadTask,
): Promise<void> => {
	const path = getTaskFullPath(task, { normalizeCompletedPath: false });
	if (!path) {
		return;
	}
	try {
		await invoke("protect_download_file", { path });
	} catch (err) {
		logger.warn(`[Motrix] protectDownloadFile failed: ${err}`);
	}
};

export const unprotectDownloadFile = async (
	task: DownloadTask,
): Promise<void> => {
	const path = getTaskFullPath(task, { normalizeCompletedPath: false });
	if (!path) {
		return;
	}
	try {
		await invoke("unprotect_download_file", { path });
	} catch (err) {
		logger.warn(`[Motrix] unprotectDownloadFile failed: ${err}`);
	}
};
