import { SELECTED_ALL_FILES } from "@shared/constants";
import type { MediaFormat } from "@shared/types/task";
import { isMediaUri } from "@shared/utils";

type BatchItemKind = "torrent" | "uri" | "magnet" | "metalink";

type BatchItemStatus = "queued" | "submitting" | "success" | "failed";

type BatchItemResolveState =
	| "idle"
	| "loading"
	| "ready"
	| "error"
	| "preview-disabled";

interface BatchItemFileSummary {
	idx: number;
	name: string;
	path: string;
	length: number;
}

export interface BatchQueueItem {
	id: string;
	kind: BatchItemKind;
	label: string;

	// torrent only
	path?: string;
	displayName?: string;
	resolveState: BatchItemResolveState;
	resolveError?: string;
	previewDisabled?: boolean;
	fileCount?: number;
	totalLength?: number;

	// uri / magnet
	uri?: string;
	magnetFiles?: BatchItemFileSummary[];

	// uri only: extra mirror URLs (same file, different servers) submitted
	// alongside `uri` as one task's mirror array for parallel multi-source download
	mirrors?: string[];

	// media (yt-dlp): set when the host is in the media allowlist or the user
	// forces yt-dlp. `mediaFormatId` is the chosen yt-dlp format selector
	isMedia?: boolean;
	forceYtdlp?: boolean;
	mediaFormatId?: string;
	mediaFormats?: MediaFormat[];
	mediaInfoState?: BatchItemResolveState;
	mediaInfoError?: string;
	mediaTitle?: string;

	// shared
	selectFile: string;
	out?: string;

	// submission
	status: BatchItemStatus;
	gid?: string;
	error?: string;
}

const createFileBatchItem = (
	kind: Extract<BatchItemKind, "torrent" | "metalink">,
	resolveState: BatchItemResolveState,
	path: string,
	name?: string,
): BatchQueueItem => {
	const segs = `${path}`.split(/[/\\]/);
	const fallback = segs[segs.length - 1] || path;
	return {
		id: crypto.randomUUID(),
		kind,
		label: name || fallback,
		path,
		displayName: name || fallback,
		resolveState,
		selectFile: SELECTED_ALL_FILES,
		status: "queued",
	};
};

export const createTorrentBatchItem = (
	path: string,
	name?: string,
): BatchQueueItem => createFileBatchItem("torrent", "idle", path, name);

export const createMetalinkBatchItem = (
	path: string,
	name?: string,
): BatchQueueItem =>
	createFileBatchItem("metalink", "preview-disabled", path, name);

const TORRENT_EXTS = ["torrent"];
const METALINK_EXTS = ["meta4", "metalink"];
const extRe = (exts: string[]) => new RegExp(`\\.(${exts.join("|")})$`, "i");
const TORRENT_RE = extRe(TORRENT_EXTS);
const METALINK_RE = extRe(METALINK_EXTS);
export const TASK_FILE_RE = extRe([...TORRENT_EXTS, ...METALINK_EXTS]);

export const batchItemForFilePath = (
	path: string,
	name?: string,
): BatchQueueItem | null => {
	if (TORRENT_RE.test(path)) {
		return createTorrentBatchItem(path, name);
	}
	if (METALINK_RE.test(path)) {
		return createMetalinkBatchItem(path, name);
	}
	return null;
};

export const createUriBatchItem = (uri: string): BatchQueueItem => {
	const trimmed = uri.trim();
	const isMagnet = trimmed.toLowerCase().startsWith("magnet:");
	const label = trimmed.length > 80 ? `${trimmed.slice(0, 77)}…` : trimmed;
	const media = !isMagnet && isMediaUri(trimmed);
	return {
		id: crypto.randomUUID(),
		kind: isMagnet ? "magnet" : "uri",
		label,
		uri: trimmed,
		resolveState: isMagnet ? "idle" : "ready",
		selectFile: SELECTED_ALL_FILES,
		status: "queued",
		isMedia: media,
		mediaInfoState: "idle",
	};
};
