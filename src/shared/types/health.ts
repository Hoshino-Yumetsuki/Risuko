export type HealthStatus = "ok" | "warn" | "fail" | "skipped";

type HealthFixKind = "open-preference" | "restart-engine" | "open-log-dir";

export interface HealthFix {
	kind: HealthFixKind;
	target: string | null;
}

export interface HealthCheck {
	id: string;
	status: HealthStatus;
	message: string;
	hint: string | null;
	fix: HealthFix | null;
	details: Record<string, unknown> | null;
}

export type HealthCategoryId =
	| "general"
	| "network"
	| "bittorrent"
	| "disk"
	| "system"
	| "config"
	| "logs";

export interface HealthCategory {
	id: HealthCategoryId;
	status: HealthStatus;
	checks: HealthCheck[];
}

export interface HealthReport {
	generatedAtMs: number;
	engineRunning: boolean;
	overallStatus: HealthStatus;
	categories: HealthCategory[];
	logPath: string;
}

export interface RunHealthChecksParams {
	categories?: HealthCategoryId[];
	slowProbes?: boolean;
}
