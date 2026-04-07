<template>
  <Dialog :open="visible" @update:open="handleOpenChange">
    <DialogContent class="rss-rules-dialog" :show-close-button="true">
      <DialogHeader>
        <DialogTitle>{{ $t('rss.auto-download-rules') }}</DialogTitle>
      </DialogHeader>
      <div class="rss-rules-body">
        <!-- Existing rules -->
        <div v-if="rules.length > 0" class="rss-rules-list">
          <div v-for="rule in rules" :key="rule.id" class="rss-rule-row">
            <div class="rss-rule-info">
              <span class="rss-rule-name">{{ rule.name }}</span>
              <span class="rss-rule-pattern">
                <code>{{ rule.pattern }}</code>
                <span v-if="rule.is_regex" class="rss-rule-tag">regex</span>
              </span>
              <span v-if="rule.feed_id" class="rss-rule-scope">
                {{ feedName(rule.feed_id) }}
              </span>
              <span v-else class="rss-rule-scope">{{ $t('rss.all-feeds') }}</span>
            </div>
            <div class="rss-rule-actions">
              <Switch :model-value="rule.is_active" @update:model-value="toggleRule(rule)" />
              <Button size="icon-sm" variant="ghost" @click="removeRule(rule.id)">
                <Trash2 :size="14" />
              </Button>
            </div>
          </div>
        </div>
        <div v-else class="rss-rules-empty">
          <p>{{ $t('rss.no-rules') }}</p>
        </div>

        <!-- Add rule form -->
        <div class="rss-rule-form">
          <h5 class="rss-rule-form-title">{{ $t('rss.add-rule') }}</h5>
          <div class="rss-rule-form-fields">
            <Input v-model="form.name" :placeholder="$t('rss.rule-name')" />
            <div class="rss-rule-pattern-row">
              <Input v-model="form.pattern" :placeholder="$t('rss.rule-pattern')" class="flex-1" />
              <label class="rss-rule-regex-label">
                <Checkbox :model-value="form.is_regex" @update:model-value="form.is_regex = $event" />
                <span>Regex</span>
              </label>
            </div>
            <select v-model="form.feed_id" class="rss-rule-select">
              <option :value="null">{{ $t('rss.all-feeds') }}</option>
              <option v-for="feed in feeds" :key="feed.id" :value="feed.id">
                {{ feed.title }}
              </option>
            </select>
          </div>
          <Button
            size="sm"
            :disabled="!form.name.trim() || !form.pattern.trim()"
            @click="handleAddRule"
          >
            {{ $t('rss.add-rule') }}
          </Button>
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" @click="handleClose">
          {{ $t('rss.close') }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import type { RssRule } from "@shared/types/rss";
import { Trash2 } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { useRssStore } from "@/store/rss";

const emptyForm = () => ({
	name: "",
	pattern: "",
	is_regex: false,
	feed_id: null as string | null,
});

export default {
	name: "mo-rss-rules-dialog",
	components: {
		Button,
		Checkbox,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		Input,
		Switch,
		Trash2,
	},
	props: {
		visible: { type: Boolean, default: false },
	},
	emits: ["close"],
	data() {
		return {
			form: emptyForm(),
		};
	},
	computed: {
		feeds() {
			return useRssStore().feeds;
		},
		rules() {
			return useRssStore().rules;
		},
	},
	watch: {
		visible(val) {
			if (val) {
				this.form = emptyForm();
				useRssStore().fetchRules();
			}
		},
	},
	methods: {
		feedName(feedId: string): string {
			const feed = useRssStore().feeds.find((f) => f.id === feedId);
			return feed?.title ?? feedId;
		},
		async handleAddRule() {
			const { name, pattern, is_regex, feed_id } = this.form;
			if (!name.trim() || !pattern.trim()) {
				return;
			}
			await useRssStore().addRule({
				name: name.trim(),
				pattern: pattern.trim(),
				is_regex,
				feed_id,
				is_active: true,
				auto_download: true,
				download_dir: null,
			});
			this.form = emptyForm();
		},
		async removeRule(ruleId: string) {
			await useRssStore().removeRule(ruleId);
		},
		async toggleRule(rule: RssRule) {
			// Remove and re-add with toggled state — simplified approach
			// A proper update endpoint would be better but this works
			await useRssStore().removeRule(rule.id);
			await useRssStore().addRule({
				...rule,
				is_active: !rule.is_active,
			});
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
.rss-rules-body {
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-height: 400px;
  overflow-y: auto;
}

.rss-rules-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.rss-rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px;
  border-radius: 4px;
  background: hsl(var(--muted) / 0.5);
}

.rss-rule-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rss-rule-name {
  font-size: 13px;
  font-weight: 500;
}

.rss-rule-pattern {
  font-size: 12px;
  color: hsl(var(--muted-foreground));
  display: flex;
  align-items: center;
  gap: 4px;
}

.rss-rule-pattern code {
  background: hsl(var(--muted));
  padding: 1px 4px;
  border-radius: 2px;
  font-size: 11px;
}

.rss-rule-tag {
  font-size: 10px;
  background: hsl(var(--primary) / 0.1);
  color: hsl(var(--primary));
  padding: 0 4px;
  border-radius: 2px;
}

.rss-rule-scope {
  font-size: 11px;
  color: hsl(var(--muted-foreground));
}

.rss-rule-actions {
  display: flex;
  align-items: center;
  gap: 4px;
}

.rss-rules-empty {
  text-align: center;
  color: hsl(var(--muted-foreground));
  font-size: 13px;
  padding: 12px;
}

.rss-rule-form {
  border-top: 1px solid hsl(var(--border));
  padding-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.rss-rule-form-title {
  font-size: 13px;
  font-weight: 600;
}

.rss-rule-form-fields {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.rss-rule-pattern-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.rss-rule-regex-label {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  white-space: nowrap;
  cursor: pointer;
}

.rss-rule-select {
  width: 100%;
  height: 32px;
  padding: 0 8px;
  font-size: 13px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  background: transparent;
  color: inherit;
}
</style>
