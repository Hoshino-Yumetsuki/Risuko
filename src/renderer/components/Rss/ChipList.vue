<template>
  <div class="chip-list">
    <label class="chip-list-label">{{ label }}</label>
    <div class="chip-list-row">
      <div v-for="(c, idx) in chips" :key="idx" class="chip">
        <span>{{ c }}</span>
        <button
          v-if="reorder && idx > 0"
          type="button"
          class="chip-arrow"
          @click="move(idx, idx - 1)"
          aria-label="up"
        >
          <ArrowLeft :size="10" />
        </button>
        <button
          v-if="reorder && idx < chips.length - 1"
          type="button"
          class="chip-arrow"
          @click="move(idx, idx + 1)"
          aria-label="down"
        >
          <ArrowRight :size="10" />
        </button>
        <button type="button" class="chip-remove" @click="remove(idx)" aria-label="remove">
          <X :size="10" />
        </button>
      </div>
      <input
        v-model="draft"
        :placeholder="placeholder"
        class="chip-input"
        @keydown.enter.prevent="commit"
        @keydown.delete="onBackspace"
        @blur="commit"
      />
    </div>
  </div>
</template>

<script lang="ts">
import { ArrowLeft, ArrowRight, X } from "@lucide/vue";

export default {
	name: "mo-rss-chip-list",
	components: { ArrowLeft, ArrowRight, X },
	props: {
		label: { type: String, required: true },
		chips: { type: Array as () => string[], required: true },
		placeholder: { type: String, default: "" },
		reorder: { type: Boolean, default: false },
	},
	emits: ["update"],
	data() {
		return { draft: "" };
	},
	methods: {
		emit(next: string[]) {
			this.$emit("update", next);
		},
		commit() {
			const value = this.draft.trim();
			if (!value) {
				return;
			}
			if (this.chips.includes(value)) {
				this.draft = "";
				return;
			}
			this.emit([...this.chips, value]);
			this.draft = "";
		},
		remove(idx: number) {
			this.emit(this.chips.filter((_, i) => i !== idx));
		},
		move(from: number, to: number) {
			const next = this.chips.slice();
			const [moved] = next.splice(from, 1);
			if (moved !== undefined) {
				next.splice(to, 0, moved);
			}
			this.emit(next);
		},
		onBackspace(e: KeyboardEvent) {
			if (this.draft === "" && this.chips.length > 0) {
				e.preventDefault();
				this.emit(this.chips.slice(0, -1));
			}
		},
	},
};
</script>

<style scoped>
.chip-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.chip-list-label {
  font-size: 12px;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
}

.chip-list-row {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding: 6px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  min-height: 36px;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px 2px 8px;
  background: hsl(var(--accent));
  border-radius: 12px;
  font-size: 12px;
}

.chip-arrow,
.chip-remove {
  border: none;
  background: transparent;
  cursor: pointer;
  padding: 2px;
  border-radius: 50%;
  color: hsl(var(--muted-foreground));
  display: flex;
  align-items: center;
  justify-content: center;
}

.chip-arrow:hover,
.chip-remove:hover {
  background: hsl(var(--background));
  color: hsl(var(--foreground));
}

.chip-input {
  flex: 1;
  min-width: 100px;
  border: none;
  outline: none;
  background: transparent;
  font-size: 13px;
  color: inherit;
  border-radius: 4px;
}

.chip-input:focus-visible {
  outline: 2px solid hsl(var(--ring));
  outline-offset: 2px;
  box-shadow: 0 0 0 2px hsl(var(--background));
}
</style>
