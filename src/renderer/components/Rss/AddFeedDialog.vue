<template>
  <Dialog :open="visible" @update:open="handleOpenChange">
    <DialogContent class="rss-add-feed-dialog" :show-close-button="true">
      <DialogHeader>
        <DialogTitle>{{ $t('rss.add-feed') }}</DialogTitle>
      </DialogHeader>
      <div class="rss-add-feed-body">
        <div class="rss-add-feed-input-row">
          <Input
            ref="urlInput"
            v-model="url"
            :placeholder="$t('rss.feed-url-placeholder')"
            :disabled="loading"
            @keydown.enter="handleAdd"
          />
        </div>
        <div v-if="error" class="rss-add-feed-error">
          {{ error }}
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" :disabled="loading" @click="handleClose">
          {{ $t('rss.cancel') }}
        </Button>
        <Button :disabled="!url.trim() || loading" @click="handleAdd">
          <RefreshCw v-if="loading" :size="14" class="animate-spin mr-1" />
          {{ $t('rss.subscribe') }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import { RefreshCw } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useRssStore } from "@/store/rss";

export default {
	name: "mo-rss-add-feed-dialog",
	components: {
		Button,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		Input,
		RefreshCw,
	},
	props: {
		visible: { type: Boolean, default: false },
	},
	emits: ["close"],
	data() {
		return {
			url: "",
			loading: false,
			error: "",
		};
	},
	watch: {
		visible(val) {
			if (val) {
				this.url = "";
				this.error = "";
			}
		},
	},
	methods: {
		async handleAdd() {
			const url = this.url.trim();
			if (!url) {
				return;
			}

			this.loading = true;
			this.error = "";
			try {
				await useRssStore().addFeed(url);
				this.$emit("close");
			} catch (e: unknown) {
				this.error =
					typeof e === "string"
						? e
						: e instanceof Error
							? e.message
							: String(e);
			} finally {
				this.loading = false;
			}
		},
		handleOpenChange(open: boolean) {
			if (!open) {
				this.handleClose();
			}
		},
		handleClose() {
			this.$emit("close");
		},
	},
};
</script>

<style scoped>
.rss-add-feed-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px 0;
}

.rss-add-feed-error {
  font-size: 12px;
  color: hsl(var(--destructive));
}
</style>
