<template>
  <div
    class="batch-item-card group relative overflow-hidden rounded-md border border-border bg-card/40"
    role="listitem"
  >
    <AccordionItem :value="item.id" class="border-none">
      <div class="flex items-center gap-2 px-3">
        <AccordionTrigger
          class="flex-1 py-3 hover:no-underline"
        >
          <div class="flex items-center gap-2 min-w-0 flex-1">
            <component :is="kindIcon" :size="14" class="shrink-0 text-muted-foreground" />
            <span class="truncate text-sm" :title="item.label">{{ item.label }}</span>
            <span
              v-if="statusChip"
              class="ml-auto shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium"
              :class="statusChipClass"
            >
              {{ statusChip }}
            </span>
          </div>
        </AccordionTrigger>
        <button
          type="button"
          class="shrink-0 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
          :aria-label="$t('task.batch-remove-item')"
          :disabled="disabled"
          @click.stop="$emit('remove', item.id)"
        >
          <X :size="14" />
        </button>
      </div>
      <AccordionContent class="px-3 pb-3 pt-0">
        <div v-if="item.kind === 'torrent' && item.path">
          <mo-select-torrent
            :torrent-path="item.path"
            :torrent-name="item.displayName || ''"
            hide-empty-drop
            hide-trash
            @change="onTorrentChange"
          />
        </div>
        <div v-else-if="item.kind === 'magnet' && item.uri">
          <mo-magnet-files
            :magnet-uri="item.uri"
            @change="onMagnetSelectionChange"
          />
        </div>
        <div v-else class="rounded-md border border-dashed border-border/60 px-3 py-4 text-xs text-muted-foreground">
          <div class="break-all font-mono">{{ item.uri }}</div>
          <div class="mt-2">{{ $t('task.batch-uri-body') }}</div>
        </div>
        <div v-if="item.error" class="mt-2 text-xs text-destructive">{{ item.error }}</div>
      </AccordionContent>
    </AccordionItem>
  </div>
</template>

<script lang="ts">
import { FileArchive, Link2, Magnet, X } from "lucide-vue-next";
import MagnetFiles from "@/components/Task/MagnetFiles.vue";
import SelectTorrent from "@/components/Task/SelectTorrent.vue";
import {
	AccordionContent,
	AccordionItem,
	AccordionTrigger,
} from "@/components/ui/accordion";
import type { BatchQueueItem } from "@/store/batchQueue";

export default {
	name: "mo-batch-item-card",
	components: {
		[SelectTorrent.name]: SelectTorrent,
		[MagnetFiles.name]: MagnetFiles,
		AccordionItem,
		AccordionTrigger,
		AccordionContent,
		X,
	},
	props: {
		item: {
			type: Object as () => BatchQueueItem,
			required: true,
		},
		disabled: {
			type: Boolean,
			default: false,
		},
	},
	emits: ["remove", "update:selectFile"],
	computed: {
		kindIcon() {
			if (this.item.kind === "torrent") {
				return FileArchive;
			}
			if (this.item.kind === "magnet") {
				return Magnet;
			}
			return Link2;
		},
		statusChip(): string {
			switch (this.item.status) {
				case "submitting":
					return this.$t("task.loading-add-task");
				case "success":
					return "OK";
				case "failed":
					return "!";
				default:
					return "";
			}
		},
		statusChipClass() {
			switch (this.item.status) {
				case "success":
					return "bg-green-500/15 text-green-600 dark:text-green-400";
				case "failed":
					return "bg-destructive/15 text-destructive";
				case "submitting":
					return "bg-muted text-muted-foreground";
				default:
					return "bg-muted/60 text-muted-foreground";
			}
		},
	},
	methods: {
		onTorrentChange(_path: string, selectFile: string) {
			this.$emit("update:selectFile", this.item.id, selectFile);
		},
		onMagnetSelectionChange(selectFile: string) {
			this.$emit("update:selectFile", this.item.id, selectFile);
		},
	},
};
</script>

<style scoped>
.batch-item-card {
  transition: border-color 0.15s ease;
}
.batch-item-card:hover {
  border-color: hsl(var(--border));
}
</style>
