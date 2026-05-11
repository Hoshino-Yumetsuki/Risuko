<template>
  <div class="ep-selector">
    <select :value="kind" class="ep-select" @change="(e: Event) => changeKind((e.target as HTMLSelectElement).value as Kind)">
      <option value="none">All / no filter</option>
      <option value="from">From episode N</option>
      <option value="range">Range N..M</option>
      <option value="specific">Specific list</option>
    </select>
    <template v-if="kind === 'from'">
      <NumberInput
        :model-value="from"
        :min="1"
        @update:model-value="(v: number) => emitSelector({ type: 'from', start: v })"
      />
    </template>
    <template v-else-if="kind === 'range'">
      <NumberInput
        :model-value="rangeStart"
        :min="1"
        @update:model-value="(v: number) => emitSelector({ type: 'range', start: v, end: Math.max(v, rangeEnd) })"
      />
      <span>-</span>
      <NumberInput
        :model-value="rangeEnd"
        :min="rangeStart"
        @update:model-value="(v: number) => emitSelector({ type: 'range', start: rangeStart, end: v })"
      />
    </template>
    <template v-else-if="kind === 'specific'">
      <Input
        :model-value="specificText"
        placeholder="1, 5, 12"
        @update:model-value="(v: string | number) => updateSpecific(String(v))"
      />
    </template>
  </div>
</template>

<script lang="ts">
import type { EpisodeSelector } from "@shared/types/rss";
import { Input } from "@/components/ui/input";
import NumberInput from "@/components/ui/NumberInput.vue";

type Kind = "none" | "from" | "range" | "specific";

export default {
	name: "mo-rss-episode-selector",
	components: { Input, NumberInput },
	props: {
		selector: { type: Object as () => EpisodeSelector | null, default: null },
	},
	emits: ["update"],
	computed: {
		kind(): Kind {
			if (!this.selector || this.selector.type === "all") {
				return "none";
			}
			return this.selector.type;
		},
		from(): number {
			return this.selector?.type === "from" ? this.selector.start : 1;
		},
		rangeStart(): number {
			return this.selector?.type === "range" ? this.selector.start : 1;
		},
		rangeEnd(): number {
			return this.selector?.type === "range" ? this.selector.end : 12;
		},
		specificText(): string {
			return this.selector?.type === "specific"
				? this.selector.values.join(", ")
				: "";
		},
	},
	methods: {
		changeKind(kind: Kind) {
			switch (kind) {
				case "none":
					this.emitSelector(null);
					break;
				case "from":
					this.emitSelector({ type: "from", start: 1 });
					break;
				case "range":
					this.emitSelector({ type: "range", start: 1, end: 12 });
					break;
				case "specific":
					this.emitSelector({ type: "specific", values: [] });
					break;
			}
		},
		emitSelector(value: EpisodeSelector | null) {
			this.$emit("update", value);
		},
		updateSpecific(text: string) {
			const values = text
				.split(/\s*,\s*|\s+/)
				.map((s) => s.trim())
				.filter((tok) => /^\d+$/.test(tok))
				.map((tok) => Number(tok))
				.filter((n) => Number.isInteger(n) && n > 0);
			this.emitSelector({ type: "specific", values });
		},
	},
};
</script>

<style scoped>
.ep-selector {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ep-select {
  height: 32px;
  padding: 0 8px;
  font-size: 13px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  background: transparent;
  color: inherit;
}
</style>
