import { SELECTED_ALL_FILES } from "@shared/constants";

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

	// shared
	selectFile: string;
	out?: string;

	// submission
	status: BatchItemStatus;
	gid?: string;
	error?: string;
}

let counter = 0;
const createBatchItemId = () => {
	counter += 1;
	return `bq-${Date.now().toString(36)}-${counter.toString(36)}`;
};

export const createTorrentBatchItem = (
	path: string,
	name?: string,
): BatchQueueItem => {
	const segs = `${path}`.split(/[/\\]/);
	const fallback = segs[segs.length - 1] || path;
	return {
		id: createBatchItemId(),
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
	return {
		id: createBatchItemId(),
		kind: isMagnet ? "magnet" : "uri",
		label,
		uri: trimmed,
		resolveState: isMagnet ? "idle" : "ready",
		selectFile: SELECTED_ALL_FILES,
		status: "queued",
	};
};
