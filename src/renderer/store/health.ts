import type { HealthCategoryId, HealthReport } from "@shared/types/health";
import type { LogEntry, LogFileSummary, LogLevel } from "@shared/types/log";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";

const STATUS_RANK: Record<HealthReport["overallStatus"], number> = {
	ok: 0,
	skipped: 1,
	warn: 2,
	fail: 3,
};

const calculateOverallStatus = (
	categories: HealthReport["categories"],
): HealthReport["overallStatus"] => {
	return categories.reduce<HealthReport["overallStatus"]>((worst, category) => {
		return STATUS_RANK[category.status] > STATUS_RANK[worst]
			? category.status
			: worst;
	}, "ok");
};

export const useHealthStore = defineStore("health", {
	state: () => ({
		report: null as HealthReport | null,
		loading: false,
		loadingCategories: new Set<HealthCategoryId>(),
		lastError: null as string | null,
		logFiles: [] as LogFileSummary[],
		logEntries: [] as LogEntry[],
		logFileName: null as string | null,
		logsLoading: false,
		logsError: null as string | null,
		logsTruncated: false,
	}),

	actions: {
		async fetchAll(slowProbes = false) {
			this.loading = true;
			this.lastError = null;
			try {
				this.report = await api.runHealthChecks({ slowProbes });
			} catch (err) {
				const msg = err instanceof Error ? err.message : String(err);
				this.lastError = msg;
				logger.error("[health] fetchAll failed", err);
			} finally {
				this.loading = false;
			}
		},

		async fetchCategory(id: HealthCategoryId) {
			this.loadingCategories.add(id);
			this.lastError = null;
			try {
				const partial = await api.runHealthChecks({ categories: [id] });
				const updated = partial.categories.find((c) => c.id === id);
				if (this.report && updated) {
					const categories = this.report.categories.map((c) =>
						c.id === id ? updated : c,
					);
					this.report = {
						...this.report,
						generatedAtMs: partial.generatedAtMs,
						engineRunning: partial.engineRunning,
						logPath: partial.logPath,
						overallStatus: calculateOverallStatus(categories),
						categories,
					};
				} else {
					this.report = partial;
				}
			} catch (err) {
				const msg = err instanceof Error ? err.message : String(err);
				this.lastError = msg;
				logger.error("[health] fetchCategory failed", err);
			} finally {
				this.loadingCategories.delete(id);
			}
		},

		async fetchLogFiles() {
			this.logsLoading = true;
			this.logsError = null;
			try {
				this.logFiles = await api.listLogFiles();
				if (
					this.logFileName &&
					!this.logFiles.some((file) => file.name === this.logFileName)
				) {
					this.logFileName = null;
				}
				if (!this.logFileName) {
					this.logFileName = this.logFiles[0]?.name ?? null;
				}
			} catch (err) {
				this.logsError = err instanceof Error ? err.message : String(err);
				logger.error("[health] fetchLogFiles failed", err);
			} finally {
				this.logsLoading = false;
			}
		},

		async readLogFile(name: string, levels?: LogLevel[]) {
			if (!name) {
				this.logEntries = [];
				this.logFileName = null;
				this.logsTruncated = false;
				return;
			}
			this.logsLoading = true;
			this.logsError = null;
			try {
				const result = await api.readLogFile({ name, levels });
				this.logFileName = result.name || name;
				this.logEntries = Array.isArray(result.entries) ? result.entries : [];
				this.logsTruncated = !!result.truncated;
			} catch (err) {
				this.logsError = err instanceof Error ? err.message : String(err);
				this.logEntries = [];
				this.logsTruncated = false;
				logger.error("[health] readLogFile failed", err);
			} finally {
				this.logsLoading = false;
			}
		},

		clear() {
			this.report = null;
			this.lastError = null;
			this.loading = false;
			this.loadingCategories.clear();
			this.logFiles = [];
			this.logEntries = [];
			this.logFileName = null;
			this.logsLoading = false;
			this.logsError = null;
			this.logsTruncated = false;
		},
	},
});
