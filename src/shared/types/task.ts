export interface FileUri {
	uri: string;
	status: string;
}

export interface DownloadFile {
	index: string;
	path: string;
	length: string;
	completedLength: string;
	selected: string;
	uris: FileUri[];
}

export interface BitTorrentInfo {
	info?: {
		name?: string;
	};
}

export interface DownloadTask {
	gid: string;
	status: string;
	kind?: string;
	totalLength: string;
	completedLength: string;
	downloadSpeed: string;
	uploadSpeed: string;
	connections: string;
	dir: string;
	files: DownloadFile[];
	errorCode?: string;
	errorMessage?: string;
	createdAt?: string;
	infoHash?: string;
	bittorrent?: BitTorrentInfo;
	seeder?: string;
	numSeeders?: string;
	ed2kLink?: string;
	numPeers?: string;
	m3u8Link?: string;
	pieceLength?: string;
	numPieces?: string;
	options?: Record<string, string>;
	chunkProgress?: { completedLength: string; totalLength: string }[];
}

export interface PeerInfo {
	peerId: string;
	ip: string;
	port: string;
	bitfield: string;
	amChoking: string;
	peerChoking: string;
	downloadSpeed: string;
	uploadSpeed: string;
	seeder: string;
}

export interface GlobalStat {
	numActive: string;
	numWaiting: string;
	numStopped: string;
	numStoppedTotal: string;
	downloadSpeed: string;
	uploadSpeed: string;
}

export interface EngineInfo {
	version: string;
	enabledFeatures: string[];
}

export interface LowSpeedEvaluationResult {
	strikeMap: Record<string, number>;
	recoverAtMap: Record<string, number>;
	recoverGids: string[];
}

export interface AutoRetryPlanResult {
	attemptMap: Record<string, number>;
	nextAttempt: number;
	delayMs: number;
}

export interface SyncOrderResult {
	moved: number;
	partialError: boolean;
}
