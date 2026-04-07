import type { SavedCredential } from "./credential";

/**
 * App configuration — merged from system.json and user.json.
 * Keys here are the camelCase form; the backend stores them in kebab-case.
 */
export interface AppConfig {
	locale: string;
	theme?: string;
	dir?: string;
	split?: number;
	allProxy?: string;
	cookie?: string;
	referer?: string;
	userAgent?: string;
	followTorrent?: boolean;
	newTaskShowDownloading?: boolean;
	fileCategoryDirs?: Record<string, string>;
	"ftp-user"?: string;
	"ftp-passwd"?: string;
	"sftp-private-key"?: string;
	"sftp-private-key-passphrase"?: string;
	traySpeedometer?: boolean | string;
	runMode?: number | string;
	rpcSecret?: string;
	autoCheckUpdate?: boolean;
	autoSyncTracker?: boolean;
	trackerSource?: string[];
	btTracker?: string;
	lastSyncTrackerTime?: number;
	maxOverallDownloadLimit?: number;
	maxOverallUploadLimit?: number;
	historyDirectories?: string[];
	favoriteDirectories?: string[];
	savedCredentials?: SavedCredential[];
	protocols?: {
		magnet?: boolean | string;
		thunder?: boolean | string;
		ed2k?: boolean | string;
	};
	proxy?: {
		enable?: boolean;
		server?: string;
		scope?: string[];
	};
	openAtLogin?: boolean;
	autoDetectLowSpeedTasks?: boolean;
	lowSpeedThreshold?: number;
	lowSpeedStrikeThreshold?: number;
	lowSpeedCooldownMs?: number;
	[key: string]: unknown;
}
