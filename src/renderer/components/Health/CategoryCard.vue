<template>
  <section class="health-cat" :class="`health-cat-${category.status}`">
    <header class="health-cat-header" @click="expanded = !expanded">
      <div class="health-cat-icon-wrap" :class="`health-cat-iwrap-${category.status}`">
        <component :is="statusIcon" :size="16" />
      </div>
      <div class="health-cat-meta">
        <h5 class="health-cat-title">{{ $t(`health.categories.${category.id}`) }}</h5>
        <div class="health-cat-counts">
          <span v-if="counts.ok > 0" class="health-count health-count-ok" :title="$t('health.statuses.ok')">
            <CheckCircle2 :size="12" />{{ counts.ok }}
          </span>
          <span v-if="counts.warn > 0" class="health-count health-count-warn" :title="$t('health.statuses.warn')">
            <AlertTriangle :size="12" />{{ counts.warn }}
          </span>
          <span v-if="counts.fail > 0" class="health-count health-count-fail" :title="$t('health.statuses.fail')">
            <XCircle :size="12" />{{ counts.fail }}
          </span>
          <span v-if="counts.skipped > 0" class="health-count health-count-skipped" :title="$t('health.statuses.skipped')">
            <CircleDashed :size="12" />{{ counts.skipped }}
          </span>
        </div>
      </div>
      <div class="health-cat-actions" @click.stop>
        <button
          type="button"
          class="health-cat-icon-btn"
          :disabled="loading"
          :title="$t('health.run-group')"
          @click="$emit('run')"
        >
          <RefreshCw :size="14" :class="{ 'animate-spin': loading }" />
        </button>
        <button
          type="button"
          class="health-cat-icon-btn"
					:title="expanded ? $t('health.collapse') : $t('health.expand')"
          @click="expanded = !expanded"
        >
          <ChevronDown :size="14" :class="{ 'health-cat-toggle-open': expanded }" />
        </button>
      </div>
    </header>
    <transition name="health-cat-expand">
      <ul v-if="expanded" class="health-cat-checks">
        <mo-health-check-row
          v-for="check in category.checks"
          :key="check.id"
          :check="check"
          @fix="$emit('fix', $event)"
        />
      </ul>
    </transition>
  </section>
</template>

<script lang="ts">
import type {
	HealthCategory,
	HealthFix,
	HealthStatus,
} from "@shared/types/health";
import {
	AlertTriangle,
	CheckCircle2,
	ChevronDown,
	CircleDashed,
	RefreshCw,
	XCircle,
} from "lucide-vue-next";
import type { PropType } from "vue";
import HealthCheckRow from "./CheckRow.vue";

const ICON: Record<HealthStatus, unknown> = {
	ok: CheckCircle2,
	warn: AlertTriangle,
	fail: XCircle,
	skipped: CircleDashed,
};

export default {
	name: "mo-health-category-card",
	components: {
		AlertTriangle,
		CheckCircle2,
		ChevronDown,
		CircleDashed,
		RefreshCw,
		XCircle,
		"mo-health-check-row": HealthCheckRow,
	},
	props: {
		category: {
			type: Object as PropType<HealthCategory>,
			required: true,
		},
		loading: { type: Boolean, default: false },
	},
	emits: ["run", "fix"] as unknown as {
		run: () => void;
		fix: (fix: HealthFix) => void;
	},
	data() {
		return {
			expanded: this.category.status !== "ok",
		};
	},
	computed: {
		statusIcon() {
			return ICON[this.category.status];
		},
		counts() {
			const c = { ok: 0, warn: 0, fail: 0, skipped: 0 };
			for (const x of this.category.checks) {
				c[x.status] += 1;
			}
			return c;
		},
	},
};
</script>

<style scoped>
.health-cat {
	border: 1px solid var(--border);
	border-radius: var(--radius);
	background: var(--card, var(--background));
	overflow: hidden;
	transition: border-color 150ms ease, box-shadow 150ms ease;
}
.health-cat:hover {
	border-color: color-mix(in srgb, var(--border) 60%, var(--foreground) 10%);
}
.health-cat-fail {
	border-color: color-mix(in srgb, #ef4444 50%, var(--border));
}
.health-cat-warn {
	border-color: color-mix(in srgb, #f59e0b 45%, var(--border));
}
.health-cat-header {
	display: flex;
	align-items: center;
	gap: 12px;
	padding: 12px 14px;
	cursor: pointer;
	user-select: none;
	transition: background 120ms ease;
}
.health-cat-header:hover {
	background: color-mix(in srgb, var(--muted) 35%, transparent);
}
.health-cat-icon-wrap {
	width: 32px;
	height: 32px;
	min-width: 32px;
	border-radius: calc(var(--radius) - 2px);
	display: inline-flex;
	align-items: center;
	justify-content: center;
	color: #fff;
	flex-shrink: 0;
}
.health-cat-iwrap-ok {
	background: linear-gradient(135deg, #22c55e 0%, #16a34a 100%);
}
.health-cat-iwrap-warn {
	background: linear-gradient(135deg, #f59e0b 0%, #d97706 100%);
}
.health-cat-iwrap-fail {
	background: linear-gradient(135deg, #ef4444 0%, #dc2626 100%);
}
.health-cat-iwrap-skipped {
	background: linear-gradient(135deg, #94a3b8 0%, #64748b 100%);
}
.health-cat-meta {
	flex: 1;
	min-width: 0;
	display: flex;
	flex-direction: column;
	gap: 4px;
}
.health-cat-title {
	margin: 0;
	font-size: 13px;
	font-weight: 600;
	color: var(--foreground);
	letter-spacing: -0.01em;
}
.health-cat-counts {
	display: flex;
	flex-wrap: wrap;
	gap: 6px;
}
.health-count {
	display: inline-flex;
	align-items: center;
	gap: 4px;
	padding: 2px 8px;
	border-radius: 999px;
	font-size: 11px;
	font-weight: 500;
	font-variant-numeric: tabular-nums;
	line-height: 1.4;
}
.health-count-ok {
	background: color-mix(in srgb, #22c55e 14%, transparent);
	color: #15803d;
}
.health-count-warn {
	background: color-mix(in srgb, #f59e0b 18%, transparent);
	color: #b45309;
}
.health-count-fail {
	background: color-mix(in srgb, #ef4444 18%, transparent);
	color: #b91c1c;
}
.health-count-skipped {
	background: color-mix(in srgb, var(--muted) 60%, transparent);
	color: var(--muted-foreground);
}
.health-cat-actions {
	display: flex;
	align-items: center;
	gap: 2px;
}
.health-cat-icon-btn {
	background: transparent;
	border: none;
	cursor: pointer;
	padding: 6px;
	border-radius: 6px;
	color: var(--muted-foreground);
	display: inline-flex;
	align-items: center;
	transition: background 120ms ease, color 120ms ease, transform 120ms ease;
}
.health-cat-icon-btn:hover:not(:disabled) {
	background: color-mix(in srgb, var(--muted) 55%, transparent);
	color: var(--foreground);
}
.health-cat-icon-btn:active:not(:disabled) {
	transform: scale(0.92);
}
.health-cat-icon-btn:disabled {
	opacity: 0.5;
	cursor: not-allowed;
}
.health-cat-toggle-open {
	transform: rotate(180deg);
	transition: transform 180ms ease;
}
.health-cat-checks {
	list-style: none;
	margin: 0;
	padding: 0;
	border-top: 1px solid var(--border);
	background: color-mix(in srgb, var(--muted) 18%, transparent);
}
.health-cat-expand-enter-active,
.health-cat-expand-leave-active {
	transition: opacity 150ms ease;
}
.health-cat-expand-enter-from,
.health-cat-expand-leave-to {
	opacity: 0;
}
</style>
