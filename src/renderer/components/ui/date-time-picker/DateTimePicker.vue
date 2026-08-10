<script setup lang="ts">
import { ChevronLeft, ChevronRight, Clock } from "@lucide/vue";
import { computed, ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import {
	Popover,
	PopoverContent,
	PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";

const props = withDefaults(
	defineProps<{
		modelValue?: number | null;
		placeholder?: string;
		allowPast?: boolean;
	}>(),
	{ modelValue: null, placeholder: "", allowPast: false },
);
const emit = defineEmits<{ "update:modelValue": [number | null] }>();

const open = ref(false);

const startOfMonth = (d: Date) => new Date(d.getFullYear(), d.getMonth(), 1);

const defaultTime = () => {
	const d = new Date();
	d.setSeconds(0, 0);
	d.setMinutes(d.getMinutes() + 1);
	return d;
};

const commit = (d: Date) =>
	emit("update:modelValue", Math.floor(d.getTime() / 1000));

const selected = computed<Date | null>(() =>
	props.modelValue ? new Date(props.modelValue * 1000) : null,
);

const viewDate = ref(startOfMonth(selected.value ?? new Date()));
watch(
	() => props.modelValue,
	(v) => {
		if (v) viewDate.value = startOfMonth(new Date(v * 1000));
	},
);

const weekdays = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];

const monthLabel = computed(() =>
	viewDate.value.toLocaleDateString(undefined, {
		month: "long",
		year: "numeric",
	}),
);

const grid = computed(() => {
	const first = startOfMonth(viewDate.value);
	const start = new Date(first);
	start.setDate(first.getDate() - first.getDay());
	const todayStart = new Date();
	todayStart.setHours(0, 0, 0, 0);
	return Array.from({ length: 42 }, (_, i) => {
		const date = new Date(start);
		date.setDate(start.getDate() + i);
		return {
			date,
			inMonth: date.getMonth() === viewDate.value.getMonth(),
			disabled: !props.allowPast && date < todayStart,
		};
	});
});

const loadHourFormat = (): boolean => {
	const saved = localStorage.getItem("risuko.time-24h");
	if (saved === "true") return true;
	if (saved === "false") return false;
	try {
		const cycle = new Intl.DateTimeFormat(undefined, {
			hour: "numeric",
		}).resolvedOptions().hourCycle;
		return cycle === "h23" || cycle === "h24";
	} catch {
		return false;
	}
};
const hour24 = ref(loadHourFormat());
const toggleFormat = () => {
	hour24.value = !hour24.value;
	localStorage.setItem("risuko.time-24h", String(hour24.value));
};

const baseDate = () => new Date(selected.value ?? defaultTime());
const currentHours = () => (selected.value ?? defaultTime()).getHours();
const isPM = computed(() => currentHours() >= 12);

watch(open, (isOpen) => {
	if (!isOpen) return;
	if (!props.modelValue) {
		const d = defaultTime();
		commit(d);
		viewDate.value = startOfMonth(d);
	}
});

const hourField = computed({
	get() {
		const h = currentHours();
		return hour24.value ? h : h % 12 || 12;
	},
	set(v: number) {
		const raw = Number(v);
		if (!Number.isFinite(raw)) return;
		const d = baseDate();
		if (hour24.value) {
			d.setHours(Math.min(23, Math.max(0, Math.floor(raw))));
		} else {
			const twelve = Math.min(12, Math.max(1, Math.floor(raw)));
			d.setHours((twelve % 12) + (isPM.value ? 12 : 0));
		}
		commit(d);
	},
});

const minuteField = computed({
	get() {
		return (selected.value ?? defaultTime()).getMinutes();
	},
	set(v: number) {
		const raw = Number(v);
		if (!Number.isFinite(raw)) return;
		const d = baseDate();
		d.setMinutes(Math.min(59, Math.max(0, Math.floor(raw))));
		commit(d);
	},
});

const toggleAmPm = () => {
	const d = baseDate();
	const h = d.getHours();
	d.setHours(h >= 12 ? h - 12 : h + 12);
	commit(d);
};

const isSameDay = (a: Date, b: Date | null) =>
	!!b &&
	a.getFullYear() === b.getFullYear() &&
	a.getMonth() === b.getMonth() &&
	a.getDate() === b.getDate();

function pickDay(date: Date) {
	const base = selected.value ?? defaultTime();
	const next = new Date(date);
	next.setHours(base.getHours(), base.getMinutes(), 0, 0);
	commit(next);
}

const prevMonth = () => {
	viewDate.value = new Date(
		viewDate.value.getFullYear(),
		viewDate.value.getMonth() - 1,
		1,
	);
};
const nextMonth = () => {
	viewDate.value = new Date(
		viewDate.value.getFullYear(),
		viewDate.value.getMonth() + 1,
		1,
	);
};

const triggerLabel = computed(() =>
	selected.value
		? selected.value.toLocaleString(undefined, {
				year: "numeric",
				month: "short",
				day: "numeric",
				hour: "2-digit",
				minute: "2-digit",
				hour12: !hour24.value,
			})
		: props.placeholder,
);

const inputClass =
	"border-input bg-transparent focus-visible:border-ring focus-visible:ring-ring/50 h-8 w-12 rounded-md border text-center text-sm outline-none focus-visible:ring-[3px] [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none";
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <Button
        type="button"
        variant="outline"
        :class="cn('w-full justify-start font-normal', !selected && 'text-muted-foreground')"
      >
        <Clock class="size-4" />
        <span class="truncate">{{ triggerLabel }}</span>
      </Button>
    </PopoverTrigger>
    <PopoverContent class="w-auto p-3" align="start">
      <div class="flex items-center justify-between px-1 pb-2">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :title="$t('app.previous-month')"
          :aria-label="$t('app.previous-month')"
          @click="prevMonth"
        >
          <ChevronLeft class="size-4" />
        </Button>
        <span class="text-sm font-medium">{{ monthLabel }}</span>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          :title="$t('app.next-month')"
          :aria-label="$t('app.next-month')"
          @click="nextMonth"
        >
          <ChevronRight class="size-4" />
        </Button>
      </div>
      <div class="grid grid-cols-7 gap-0.5 text-center">
        <span
          v-for="w in weekdays"
          :key="w"
          class="text-muted-foreground py-1 text-xs"
        >{{ w }}</span>
        <button
          v-for="cell in grid"
          :key="cell.date.toISOString()"
          type="button"
          :disabled="cell.disabled"
          :class="cn(
            'flex h-8 w-8 items-center justify-center rounded-md text-sm transition-colors',
            'hover:bg-accent hover:text-accent-foreground',
            !cell.inMonth && 'text-muted-foreground/50',
            cell.disabled && 'pointer-events-none opacity-30',
            isSameDay(cell.date, selected) && 'bg-primary text-primary-foreground hover:bg-primary/90',
          )"
          @click="pickDay(cell.date)"
        >{{ cell.date.getDate() }}</button>
      </div>
      <div class="mt-3 flex items-center gap-1.5 border-t pt-3">
        <Clock class="text-muted-foreground size-4 shrink-0" />
        <input
          v-model.number="hourField"
          type="number"
          :min="hour24 ? 0 : 1"
          :max="hour24 ? 23 : 12"
          aria-label="Hour"
          :class="inputClass"
        />
        <span class="text-muted-foreground">:</span>
        <input
          v-model.number="minuteField"
          type="number"
          min="0"
          max="59"
          aria-label="Minute"
          :class="inputClass"
        />
        <button
          v-if="!hour24"
          type="button"
          class="h-8 rounded-md border border-input px-2.5 text-xs font-medium transition-colors hover:bg-accent hover:text-accent-foreground"
          @click="toggleAmPm"
        >{{ isPM ? 'PM' : 'AM' }}</button>
        <button
          type="button"
          class="ml-auto h-8 rounded-md border border-input px-2.5 text-xs font-medium transition-colors hover:bg-accent hover:text-accent-foreground"
          @click="toggleFormat"
        >{{ hour24 ? '24H' : '12H' }}</button>
      </div>
    </PopoverContent>
  </Popover>
</template>
