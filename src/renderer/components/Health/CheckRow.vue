<template>
  <li class="health-row" :class="`health-row-${check.status}`">
    <span class="health-row-icon">
      <component :is="icon" :size="14" />
    </span>
    <div class="health-row-body">
      <div class="health-row-message">{{ check.message }}</div>
      <div v-if="check.hint" class="health-row-hint">{{ check.hint }}</div>
    </div>
    <button
      v-if="check.fix"
      type="button"
      class="health-row-fix"
      @click="$emit('fix', check.fix)"
    >
      {{ $t(`health.fixes.${check.fix.kind}`) }}
    </button>
  </li>
</template>

<script lang="ts">
import {
	AlertTriangle,
	CheckCircle2,
	CircleDashed,
	XCircle,
} from "@lucide/vue";
import type {
	HealthCheck,
	HealthFix,
	HealthStatus,
} from "@shared/types/health";
import type { PropType } from "vue";

const ICON: Record<HealthStatus, unknown> = {
	ok: CheckCircle2,
	warn: AlertTriangle,
	fail: XCircle,
	skipped: CircleDashed,
};

export default {
	name: "mo-health-check-row",
	props: {
		check: {
			type: Object as PropType<HealthCheck>,
			required: true,
		},
	},
	emits: ["fix"] as unknown as { fix: (fix: HealthFix) => void },
	computed: {
		icon() {
			return ICON[this.check.status as HealthStatus];
		},
	},
};
</script>

<style scoped>
.health-row {
	display: flex;
	align-items: flex-start;
	gap: 10px;
	padding: 10px 14px 10px 58px;
	border-top: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
}
.health-row:first-child {
	border-top: none;
}
.health-row-icon {
	margin-top: 2px;
	display: inline-flex;
}
.health-row-ok .health-row-icon { color: #16a34a; }
.health-row-warn .health-row-icon { color: #d97706; }
.health-row-fail .health-row-icon { color: #dc2626; }
.health-row-skipped .health-row-icon { color: var(--muted-foreground); }
.health-row-body {
	flex: 1;
	min-width: 0;
}
.health-row-message {
	font-size: 13px;
	color: var(--foreground);
	overflow-wrap: anywhere;
	line-height: 1.45;
}
.health-row-hint {
	margin-top: 2px;
	font-size: 12px;
	color: var(--muted-foreground);
}
.health-row-fix {
	flex-shrink: 0;
	background: transparent;
	border: 1px solid var(--border);
	color: var(--foreground);
	font-size: 12px;
	padding: 4px 10px;
	border-radius: 6px;
	cursor: pointer;
	transition: background 120ms ease, transform 120ms ease;
}
.health-row-fix:hover {
	background: color-mix(in srgb, var(--muted) 60%, transparent);
}
.health-row-fix:active {
	transform: scale(0.96);
}
</style>
