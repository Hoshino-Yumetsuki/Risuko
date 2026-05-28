<template>
  <div class="main panel panel-layout panel-layout--v health-panel">
    <mo-enter tag="header" preset="fadeInDown" class="panel-header health-header">
      <h4 class="health-title">{{ $t('health.title') }}</h4>
      <Button
        size="sm"
        variant="default"
        class="health-run-btn"
        :class="{ 'is-loading': loading }"
        :disabled="loading"
        @click="runAll"
      >
        <RefreshCw :size="14" :class="{ 'animate-spin': loading }" />
        <span>{{ loading ? $t('health.loading') : $t('health.run-all') }}</span>
      </Button>
    </mo-enter>
    <p class="health-subtitle">{{ $t('health.subtitle') }}</p>
    <div v-if="report" class="health-meta-row">
      <span class="health-overall" :class="`health-status-${report.overallStatus}`">
        <component :is="iconFor(report.overallStatus)" :size="14" />
        {{ $t(`health.statuses.${report.overallStatus}`) }}
      </span>
      <span class="health-last-run" :title="lastRunTitle">
        {{ $t('health.last-run') }}: {{ lastRunRelative }}
      </span>
    </div>

    <main class="panel-content health-body">
      <div class="health-body-inner">
        <div v-if="lastError" class="health-error">
          <AlertCircle :size="16" />
          <span>{{ $t('health.error', { error: lastError }) }}</span>
        </div>

        <div v-if="!report && loading" class="health-loading">
          <RefreshCw :size="16" class="animate-spin" />
          <span>{{ $t('health.loading') }}</span>
        </div>

        <mo-health-category-card
          v-for="cat in report?.categories ?? []"
          :key="cat.id"
          :category="cat"
          :loading="loadingCategories.has(cat.id)"
          @run="onRunCategory(cat.id)"
          @fix="onFix"
        />
      </div>
    </main>
  </div>
</template>

<script lang="ts">
import {
	AlertCircle,
	AlertTriangle,
	CheckCircle2,
	CircleDashed,
	RefreshCw,
	XCircle,
} from "@lucide/vue";
import type {
	HealthCategoryId,
	HealthFix,
	HealthStatus,
} from "@shared/types/health";
import { invoke } from "@tauri-apps/api/core";
import { mapState } from "pinia";
import { Button } from "@/components/ui/button";
import { useHealthStore } from "@/store/health";
import HealthCategoryCard from "./CategoryCard.vue";

const STATUS_ICON: Record<HealthStatus, unknown> = {
	ok: CheckCircle2,
	warn: AlertTriangle,
	fail: XCircle,
	skipped: CircleDashed,
};

const REFRESH_INTERVAL_MS = 15_000;

export default {
	name: "mo-content-health",
	components: {
		AlertCircle,
		Button,
		RefreshCw,
		"mo-health-category-card": HealthCategoryCard,
	},
	data() {
		return {
			refreshTimer: null as number | null,
			now: Date.now(),
			tickTimer: null as number | null,
		};
	},
	computed: {
		...mapState(useHealthStore, [
			"report",
			"loading",
			"loadingCategories",
			"lastError",
		]),
		lastRunRelative(): string {
			if (!this.report) {
				return this.$t("health.never-run");
			}
			const diff = Math.max(0, this.now - this.report.generatedAtMs);
			if (diff < 5_000) {
				return this.$t("health.justNow");
			}
			if (diff < 60_000) {
				return this.$t("health.secondsAgo", {
					count: Math.floor(diff / 1_000),
				});
			}
			if (diff < 3_600_000) {
				return this.$t("health.minutesAgo", {
					count: Math.floor(diff / 60_000),
				});
			}
			return this.$t("health.hoursAgo", {
				count: Math.floor(diff / 3_600_000),
			});
		},
		lastRunTitle(): string {
			if (!this.report) {
				return "";
			}
			return new Date(this.report.generatedAtMs).toLocaleString();
		},
	},
	created() {
		const store = useHealthStore();
		store.fetchAll(false);
	},
	mounted() {
		this.startAutoRefresh();
		this.tickTimer = window.setInterval(() => {
			this.now = Date.now();
		}, 1_000);
		document.addEventListener("visibilitychange", this.onVisibilityChange);
	},
	beforeUnmount() {
		this.stopAutoRefresh();
		if (this.tickTimer != null) {
			clearInterval(this.tickTimer);
			this.tickTimer = null;
		}
		document.removeEventListener("visibilitychange", this.onVisibilityChange);
	},
	methods: {
		iconFor(status: HealthStatus) {
			return STATUS_ICON[status];
		},
		startAutoRefresh() {
			this.stopAutoRefresh();
			this.refreshTimer = window.setInterval(() => {
				if (document.hidden) {
					return;
				}
				useHealthStore().fetchAll(false);
			}, REFRESH_INTERVAL_MS);
		},
		stopAutoRefresh() {
			if (this.refreshTimer != null) {
				clearInterval(this.refreshTimer);
				this.refreshTimer = null;
			}
		},
		onVisibilityChange() {
			if (!document.hidden) {
				useHealthStore().fetchAll(false);
			}
		},
		runAll() {
			useHealthStore().fetchAll(true);
		},
		onRunCategory(id: HealthCategoryId) {
			useHealthStore().fetchCategory(id);
		},
		async onFix(fix: HealthFix) {
			switch (fix.kind) {
				case "open-preference": {
					const target = fix.target ?? "basic";
					this.$router.push({ path: `/preference/${target}` });
					break;
				}
				case "restart-engine": {
					await invoke("restart_engine").catch(() => undefined);
					await useHealthStore().fetchAll(false);
					break;
				}
				case "open-log-dir": {
					const path = useHealthStore().report?.logPath;
					if (path) {
						await invoke("reveal_in_folder", { path });
					}
					break;
				}
			}
		},
	},
};
</script>

<style scoped>
.health-panel {
	height: 100%;
	overflow: hidden;
}
.health-header {
	display: flex;
	flex-direction: row;
	align-items: center;
	justify-content: space-between;
	gap: 16px;
	padding: 16px 28px;
	flex-wrap: wrap;
	border-bottom: 1px solid var(--border);
}
.health-title {
	margin: 0;
	font-size: 16px;
	font-weight: 600;
	letter-spacing: -0.01em;
}
.health-subtitle {
	margin: 4px 28px 0;
	font-size: 12px;
	color: var(--muted-foreground);
}
.health-meta-row {
	display: flex;
	align-items: center;
	gap: 12px;
	flex-wrap: wrap;
	padding: 8px 28px 0;
}
.health-overall {
	display: inline-flex;
	align-items: center;
	gap: 6px;
	padding: 4px 10px;
	border-radius: 999px;
	font-size: 12px;
	font-weight: 500;
	border: 1px solid transparent;
}
.health-status-ok {
	background: color-mix(in srgb, var(--primary) 12%, transparent);
	color: var(--primary);
}
.health-status-warn {
	background: color-mix(in srgb, #f59e0b 18%, transparent);
	color: #b45309;
}
.health-status-fail {
	background: color-mix(in srgb, #ef4444 18%, transparent);
	color: #b91c1c;
}
.health-status-skipped {
	background: color-mix(in srgb, var(--muted) 60%, transparent);
	color: var(--muted-foreground);
}
.health-last-run {
	font-size: 12px;
	color: var(--muted-foreground);
	font-variant-numeric: tabular-nums;
}
.health-run-btn {
	transition: transform 120ms ease, opacity 120ms ease;
}
.health-run-btn:active:not(:disabled) {
	transform: scale(0.96);
}
.health-run-btn.is-loading {
	opacity: 0.85;
}
.health-body {
	overflow-y: auto;
	overflow-x: hidden;
}
.health-body-inner {
	max-width: 880px;
	margin: 0 auto;
	padding: 20px 28px 80px;
	display: flex;
	flex-direction: column;
	gap: 14px;
}
.health-error {
	display: flex;
	align-items: center;
	gap: 8px;
	padding: 12px 14px;
	border-radius: var(--radius);
	background: color-mix(in srgb, #ef4444 12%, transparent);
	color: #b91c1c;
	font-size: 13px;
}
.health-loading {
	display: flex;
	align-items: center;
	gap: 8px;
	padding: 24px;
	color: var(--muted-foreground);
	justify-content: center;
}
</style>
