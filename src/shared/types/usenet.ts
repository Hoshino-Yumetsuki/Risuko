export type UsenetSecurityMode = "implicit-tls" | "starttls" | "plain";

export type UsenetCleanupMode =
	| "keep-all"
	| "delete-par2"
	| "delete-par2-and-archives";

export interface UsenetProviderProfile {
	id: string;
	name: string;
	host: string;
	port: number;
	securityMode: UsenetSecurityMode;
	enabled: boolean;
	priority: number;
	maxConnections: number;
	allowPlain: boolean;
	deletedAt?: number;
	updatedAt?: number;
}

export interface UsenetArchiveLimits {
	maxEntries: number;
	maxExpandedBytes: number;
	maxEntryBytes: number;
	maxNestingDepth: number;
	maxCompressionRatio: number;
	freeSpaceReserveBytes: number;
	maxActiveSeconds: number;
}

export const DEFAULT_USENET_ARCHIVE_LIMITS: UsenetArchiveLimits = {
	maxEntries: 500_000,
	maxExpandedBytes: 2 * 1024 ** 4,
	maxEntryBytes: 512 * 1024 ** 3,
	maxNestingDepth: 16,
	maxCompressionRatio: 1000,
	freeSpaceReserveBytes: 10 * 1024 ** 3,
	maxActiveSeconds: 6 * 60 * 60,
};

export const ANDROID_USENET_ARCHIVE_LIMITS: UsenetArchiveLimits = {
	maxEntries: 100_000,
	maxExpandedBytes: 256 * 1024 ** 3,
	maxEntryBytes: 64 * 1024 ** 3,
	maxNestingDepth: 16,
	maxCompressionRatio: 1000,
	freeSpaceReserveBytes: 2 * 1024 ** 3,
	maxActiveSeconds: 2 * 60 * 60,
};

export interface UsenetTaskOptions {
	profileId?: string;
	archiveLimits?: Partial<UsenetArchiveLimits>;
	archiveLimitOverrideConfirmed?: boolean;
	cleanupMode?: UsenetCleanupMode;
}
