<template>
  <section class="health-cat" :class="`health-cat-${category.status}`">
    <header class="health-cat-header" @click="expanded = !expanded">
      <div class="health-cat-icon-wrap" :class="`health-cat-iwrap-${category.status}`">
        <component :is="statusIcon" :size="17" />
      </div>
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
      <div class="health-cat-actions" @click.stop>
        <button
          type="button"
          class="health-cat-icon-btn health-cat-refresh"
          :disabled="loading"
          :title="$t('health.run-group')"
          :aria-label="$t('health.run-group')"
          @click="$emit('run')"
        >
          <RefreshCw :size="14" :class="{ 'animate-spin': loading }" />
        </button>
        <button
          type="button"
          class="health-cat-icon-btn"
					:title="expanded ? $t('health.collapse') : $t('health.expand')"
					:aria-label="expanded ? $t('health.collapse') : $t('health.expand')"
          @click="expanded = !expanded"
        >
          <ChevronDown :size="14" :class="{ 'health-cat-toggle-open': expanded }" />
        </button>
      </div>
    </header>
    <transition name="health-cat-expand">
      <ul v-if="expanded" class="health-cat-checks">
        <health-check-row
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
import {
	AlertTriangle,
	CheckCircle2,
	ChevronDown,
	CircleDashed,
	RefreshCw,
	XCircle,
} from "@lucide/vue";
import type { HealthCategory, HealthFix } from "@shared/types/health";
import type { PropType } from "vue";
import HealthCheckRow, { STATUS_ICON } from "./CheckRow.vue";

export default {
	name: "health-category-card",
	components: {
		AlertTriangle,
		CheckCircle2,
		ChevronDown,
		CircleDashed,
		RefreshCw,
		XCircle,
		"health-check-row": HealthCheckRow,
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
			return STATUS_ICON[this.category.status];
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
.health-cat + .health-cat {
	border-top: 1px solid var(--border);
}
.health-cat-header {
	display: flex;
	align-items: center;
	gap: 10px;
	padding: 12px 14px;
	cursor: pointer;
	user-select: none;
	transition: background-color 120ms ease;
}
.health-cat-header:hover {
	background: color-mix(in srgb, var(--surface-2) 55%, transparent);
}
.health-cat-icon-wrap {
	display: inline-flex;
	align-items: center;
	justify-content: center;
	flex-shrink: 0;
	line-height: 0;
}
.health-cat-iwrap-ok {
	color: var(--success);
}
.health-cat-iwrap-warn {
	color: var(--warning);
}
.health-cat-iwrap-fail {
	color: var(--danger);
}
.health-cat-iwrap-skipped {
	color: var(--text-3);
}
.health-cat-title {
	margin: 0;
	font-size: 13px;
	font-weight: 600;
	color: var(--text-1);
	letter-spacing: -0.01em;
	white-space: nowrap;
	overflow: hidden;
	text-overflow: ellipsis;
}
.health-cat-counts {
	margin-left: auto;
	display: flex;
	flex-wrap: wrap;
	align-items: center;
	justify-content: flex-end;
	gap: 10px;
}
.health-count {
	display: inline-flex;
	align-items: center;
	gap: 3px;
	font-size: 11.5px;
	font-weight: 500;
	font-variant-numeric: tabular-nums;
	line-height: 1;
}
.health-count-ok {
	color: var(--text-3);
}
.health-count-warn {
	color: var(--warning);
}
.health-count-fail {
	color: var(--danger);
}
.health-count-skipped {
	color: var(--text-3);
}
.health-cat-actions {
	display: flex;
	align-items: center;
	gap: 2px;
	flex-shrink: 0;
}
.health-cat-refresh {
	opacity: 0;
	transition: opacity 120ms ease;
}
.health-cat-header:hover .health-cat-refresh,
.health-cat-refresh:focus-visible {
	opacity: 1;
}
.health-cat-icon-btn {
	background: transparent;
	border: none;
	cursor: pointer;
	padding: 5px;
	border-radius: 6px;
	color: var(--text-3);
	display: inline-flex;
	align-items: center;
	transition: background-color 120ms ease, color 120ms ease;
}
.health-cat-icon-btn:hover:not(:disabled) {
	background: var(--surface-2);
	color: var(--text-1);
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
	padding: 2px 0;
	border-top: 1px solid var(--border);
	background: var(--bg);
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
