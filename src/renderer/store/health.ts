import type { HealthCategoryId, HealthReport } from "@shared/types/health";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";

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
					this.report = {
						...partial,
						categories: this.report.categories.map((c) =>
							c.id === id ? updated : c,
						),
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
