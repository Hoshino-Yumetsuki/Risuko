export interface EngineConfig {
	/** Custom config directory (default: OS config dir / dev.risuko.app) */
	configDir?: string;
	/** RPC listen port (default: 16800) */
	rpcPort?: number;
	/** Whether to start the RPC server (default: true) */
	enableRpc?: boolean;
}

// Engine lifecycle
export function startEngine(config?: EngineConfig): Promise<void>;
export function stopEngine(): Promise<void>;

// Task operations — returns GID
export function addUri(
	uris: string[],
	options?: Record<string, unknown>,
): Promise<string>;
export function addTorrent(
	data: Buffer,
	options?: Record<string, unknown>,
): Promise<string>;
export function addMagnet(
	uri: string,
	options?: Record<string, unknown>,
): Promise<string>;
export function addEd2k(
	uri: string,
	options?: Record<string, unknown>,
): Promise<string>;
export function addM3u8(
	uri: string,
	options?: Record<string, unknown>,
): Promise<string>;
export function addFtp(
	uri: string,
	options?: Record<string, unknown>,
): Promise<string>;

// Control
export function pause(gid: string): Promise<void>;
export function unpause(gid: string): Promise<void>;
export function remove(gid: string): Promise<void>;
export function pauseAll(): Promise<void>;
export function unpauseAll(): Promise<void>;

// Query
export function tellStatus(
	gid: string,
	keys?: string[],
): Promise<Record<string, unknown>>;
export function tellActive(keys?: string[]): Promise<Record<string, unknown>[]>;
export function tellWaiting(
	offset: number,
	num: number,
	keys?: string[],
): Promise<Record<string, unknown>[]>;
export function tellStopped(
	offset: number,
	num: number,
	keys?: string[],
): Promise<Record<string, unknown>[]>;
export function getGlobalStat(): Promise<Record<string, unknown>>;
export function getFiles(gid: string): Promise<Record<string, unknown>[]>;
export function getPeers(gid: string): Promise<Record<string, unknown>[]>;
export function getUris(gid: string): Promise<Record<string, unknown>[]>;

// Options
export function getOption(gid: string): Promise<Record<string, unknown>>;
export function getGlobalOption(): Promise<Record<string, unknown>>;
export function changeOption(
	gid: string,
	options: Record<string, unknown>,
): Promise<void>;
export function changeGlobalOption(
	options: Record<string, unknown>,
): Promise<void>;

// Session
export function saveSession(): Promise<void>;
export function purgeDownloadResult(): Promise<void>;
export function removeDownloadResult(gid: string): Promise<void>;

// Events
export type EngineEventName =
	| "risuko.onDownloadStart"
	| "risuko.onDownloadPause"
	| "risuko.onDownloadStop"
	| "risuko.onDownloadComplete"
	| "risuko.onDownloadError"
	| "risuko.onBtDownloadComplete";

export function onEvent(
	callback: (eventName: EngineEventName, gid: string) => void,
): void;
