import type { HealthCategoryId, HealthReport } from "@shared/types/health";
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

		clear() {
			this.report = null;
			this.lastError = null;
			this.loading = false;
			this.loadingCategories.clear();
		},
	},
});
