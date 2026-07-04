import type { SavedCredential } from "./credential";

export const FONT_FAMILY_OPTIONS = [
	"system",
	"rounded",
	"serif",
	"mono",
] as const;
export type FontFamily = (typeof FONT_FAMILY_OPTIONS)[number];
export const DEFAULT_FONT_FAMILY: FontFamily = "system";

export const FONT_SIZE_OPTIONS = [
	"small",
	"default",
	"large",
	"extra-large",
] as const;
export type FontSize = (typeof FONT_SIZE_OPTIONS)[number];
export const DEFAULT_FONT_SIZE: FontSize = "default";

export const normalizeConfigOption = <T extends readonly string[]>(
	value: unknown,
	options: T,
	fallback: T[number],
): T[number] => {
	return typeof value === "string" && options.includes(value)
		? (value as T[number])
		: fallback;
};

interface TaskRoutingRule {
	id: string;
	label: string;
	pattern: string;
	dir: string;
	enabled: boolean;
}

/**
 * App configuration — merged from system.json and user.json.
 * Keys here are the camelCase form; the backend stores them in kebab-case
 */
export interface AppConfig {
	locale: string;
	theme?: string;
	fontFamily?: FontFamily;
	fontSize?: FontSize;
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
	engineMode?: "MAX" | "LIMIT";
	taskListStyle?: "compact" | "card";
	sidebarCollapsed?: boolean | string;
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
	shutdownWhenComplete?: boolean;
	// Clipboard Watcher — offer to download magnet/file links copied anywhere
	clipboardWatch?: boolean;
	clipboardWatchExtensions?: string[];
	clipboardWatchNoticeSeen?: boolean;
	// First-launch legal acceptance gate
	legalAccepted?: boolean;
	// DNS over HTTPS
	dohEnable?: boolean;
	dohUrl?: string;
	dohBootstrap?: string;
	dohFallback?: boolean;
	dohProvider?: string;
	cloudSyncEnabled?: boolean;
	cloudSyncAuto?: boolean;
	cloudSyncCategories?: string[];
	cloudSyncToken?: string;
	cloudSyncLastAt?: number;
	cloudSyncServerUrl?: string;
	cloudSyncCategoryTimestamps?: Record<string, number>;
	[key: string]: unknown;
}
