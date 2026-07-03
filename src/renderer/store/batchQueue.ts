import { SELECTED_ALL_FILES } from "@shared/constants";
import type { MediaFormat } from "@shared/types/task";
import { isMediaUri } from "@shared/utils";

type BatchItemKind = "torrent" | "uri" | "magnet";

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

export const createTorrentBatchItem = (
	path: string,
	name?: string,
): BatchQueueItem => {
	const segs = `${path}`.split(/[/\\]/);
	const fallback = segs[segs.length - 1] || path;
	return {
		id: crypto.randomUUID(),
		kind: "torrent",
		label: name || fallback,
		path,
		displayName: name || fallback,
		resolveState: "idle",
		selectFile: SELECTED_ALL_FILES,
		status: "queued",
	};
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
