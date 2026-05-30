import type { SavedCredential } from "./credential";

export interface TaskRoutingRule {
	id: string;
	label: string;
	pattern: string;
	dir: string;
	enabled: boolean;
}

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
	externalEngineEnabled?: boolean | string;
	externalEngineHost?: string;
	externalEnginePort?: number | string;
	externalEngineSecret?: string;
	engineOverrides?: Record<string, string | number | boolean | null>;
	autoCheckUpdate?: boolean;
	autoSyncTracker?: boolean;
	trackerSource?: string[];
	btTracker?: string;
	btCreateSubfolder?: boolean;
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
		adc?: boolean | string;
		gnutella?: boolean | string;
		g2?: boolean | string;
	};
	proxy?: {
		enable?: boolean;
		server?: string;
		scope?: string[];
	};
	openAtLogin?: boolean;
	preventSleepWhileDownloading?: boolean;
	purgeRecordOnStart?: boolean;
	autoDetectLowSpeedTasks?: boolean;
	lowSpeedThreshold?: number;
	lowSpeedStrikeThreshold?: number;
	lowSpeedCooldownMs?: number;
	appLogPath?: string;
	logDirOverride?: string;
	taskRoutingRules?: TaskRoutingRule[];
	completionScriptEnabled?: boolean;
	completionScriptCommand?: string;
	completionScriptArgs?: string;
	completionScriptTimeoutMs?: number;
	// DNS over HTTPS
	dohEnable?: boolean;
	dohUrl?: string;
	dohBootstrap?: string;
	dohFallback?: boolean;
	dohProvider?: string;
	[key: string]: unknown;
}
