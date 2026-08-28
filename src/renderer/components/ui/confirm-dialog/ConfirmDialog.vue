<script setup lang="ts">
import { TriangleAlert } from "@lucide/vue";
import { ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import UiCheckbox from "@/components/ui/compat/UiCheckbox.vue";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";

const props = withDefaults(
	defineProps<{
		open: boolean;
		title?: string;
		message?: string;
		kind?: "info" | "warning";
		confirmText?: string;
		cancelText?: string;
		checkboxLabel?: string;
		checkboxChecked?: boolean;
	}>(),
	{
		title: "",
		message: "",
		kind: "info",
		confirmText: "OK",
		cancelText: "Cancel",
		checkboxLabel: "",
		checkboxChecked: false,
	},
);

const emit = defineEmits<{
	confirm: [checkboxChecked: boolean];
	cancel: [];
	"update:open": [value: boolean];
}>();

const checked = ref(props.checkboxChecked);

watch(
	() => props.checkboxChecked,
	(val) => {
		checked.value = val;
	},
);

watch(
	() => props.open,
	(val) => {
		if (val) {
			checked.value = props.checkboxChecked;
		}
	},
);

function onConfirm() {
	emit("confirm", checked.value);
	emit("update:open", false);
}

function onCancel() {
	emit("cancel");
	emit("update:open", false);
}

function onOpenChange(val: boolean) {
	if (!val) {
		onCancel();
	}
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpenChange">
    <DialogContent
      :show-close-button="false"
      class="w-[min(25rem,calc(100vw-2rem))] max-w-[calc(100vw-2rem)] gap-0 overflow-hidden p-0 sm:max-w-100"
    >
      <div class="flex min-w-0 flex-col gap-3 p-5 pb-4">
        <DialogHeader
          class="min-w-0 flex-row! items-center text-left! gap-3 space-y-0"
        >
          <div
            v-if="kind === 'warning'"
            class="flex size-9 shrink-0 items-center justify-center rounded-md bg-amber-500/10"
          >
            <TriangleAlert class="size-5 text-amber-500" />
          </div>
          <DialogTitle class="min-w-0 wrap-break-word text-[15px] font-semibold leading-snug">
            {{ title }}
          </DialogTitle>
        </DialogHeader>
        <p class="min-w-0 wrap-anywhere text-[13px] leading-relaxed text-muted-foreground">
          {{ message }}
        </p>
        <UiCheckbox
          v-if="checkboxLabel"
          v-model="checked"
          class="pt-1"
        >
          <span class="min-w-0 wrap-break-word text-[13px] font-normal">
            {{ checkboxLabel }}
          </span>
        </UiCheckbox>
      </div>
      <DialogFooter class="w-full flex-row flex-wrap justify-end gap-2 border-t px-5 py-3">
        <Button variant="outline" size="sm" @click="onCancel">
          {{ cancelText }}
        </Button>
        <Button
          :variant="kind === 'warning' ? 'destructive' : 'default'"
          size="sm"
          @click="onConfirm"
        >
          {{ confirmText }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
