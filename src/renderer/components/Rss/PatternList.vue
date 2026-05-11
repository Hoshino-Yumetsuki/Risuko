<template>
  <div class="pattern-list">
    <div class="pattern-list-header">
      <label class="pattern-list-title">{{ title }}</label>
      <Button size="sm" variant="ghost" @click="addPattern">
        <Plus :size="14" />
      </Button>
    </div>
    <div v-if="patterns.length === 0" class="pattern-list-empty">—</div>
    <div v-for="(p, idx) in patterns" :key="idx" class="pattern-row">
      <select
        :value="p.kind"
        class="pattern-select"
        @change="(e: Event) => updateKind(idx, (e.target as HTMLSelectElement).value as PatternKind)"
      >
        <option value="contains">contains</option>
        <option value="glob">glob</option>
        <option value="regex">regex</option>
      </select>
      <Input
        :model-value="p.value"
        class="pattern-input"
        @update:model-value="(v: string | number) => updateValue(idx, String(v))"
      />
      <label class="pattern-case">
        <Checkbox
          :model-value="p.case_sensitive"
          @update:model-value="(v: boolean) => updateCase(idx, v)"
        />
        <span>Aa</span>
      </label>
      <Button size="icon-sm" variant="ghost" @click="removePattern(idx)">
        <X :size="14" />
      </Button>
    </div>
  </div>
</template>

<script lang="ts">
import type { Pattern, PatternKind } from "@shared/types/rss";
import { Plus, X } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";

export default {
	name: "mo-rss-pattern-list",
	components: { Button, Checkbox, Input, Plus, X },
	props: {
		title: { type: String, required: true },
		patterns: { type: Array as () => Pattern[], required: true },
	},
	emits: ["update"],
	methods: {
		emit(next: Pattern[]) {
			this.$emit("update", next);
		},
		addPattern() {
			this.emit([
				...this.patterns,
				{ value: "", kind: "contains", case_sensitive: false },
			]);
		},
		removePattern(idx: number) {
			this.emit(this.patterns.filter((_, i) => i !== idx));
		},
		updateValue(idx: number, value: string) {
			const next = this.patterns.map((p, i) =>
				i === idx ? { ...p, value } : p,
			);
			this.emit(next);
		},
		updateKind(idx: number, kind: PatternKind) {
			const next = this.patterns.map((p, i) =>
				i === idx ? { ...p, kind } : p,
			);
			this.emit(next);
		},
		updateCase(idx: number, case_sensitive: boolean) {
			const next = this.patterns.map((p, i) =>
				i === idx ? { ...p, case_sensitive } : p,
			);
			this.emit(next);
		},
	},
};
</script>

<style scoped>
.pattern-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.pattern-list-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.pattern-list-title {
  font-size: 12px;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
}

.pattern-list-empty {
  font-size: 12px;
  color: hsl(var(--muted-foreground));
}

.pattern-row {
  display: grid;
  grid-template-columns: 90px 1fr auto auto;
  gap: 6px;
  align-items: center;
}

.pattern-select {
  height: 32px;
  padding: 0 6px;
  font-size: 12px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  background: transparent;
  color: inherit;
}

.pattern-case {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
}
</style>
