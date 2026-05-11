<template>
  <Dialog :open="visible" @update:open="handleOpenChange">
    <DialogContent class="rss-rules-dialog" :show-close-button="true">
      <DialogHeader>
        <DialogTitle>{{ $t('rss.auto-download-rules') }}</DialogTitle>
      </DialogHeader>

      <div class="rss-rules-layout">
        <!-- Master pane: rule list -->
        <div class="rss-rules-master">
          <div class="rss-rules-master-toolbar">
            <Button size="sm" variant="outline" @click="createDraft">
              <Plus :size="14" />
              <span>{{ $t('rss.add-rule') }}</span>
            </Button>
          </div>
          <div v-if="rules.length === 0" class="rss-rules-empty">
            {{ $t('rss.no-rules') }}
          </div>
          <ul v-else class="rss-rules-list" role="listbox">
            <li
              v-for="(rule, idx) in rules"
              :key="rule.id || `draft-${idx}`"
              :class="['rss-rule-row', { active: selectedId === (rule.id || `draft-${idx}`) }]"
              role="option"
              tabindex="0"
              :aria-selected="selectedId === (rule.id || `draft-${idx}`)"
              @click="selectRule(rule.id || `draft-${idx}`)"
              @keydown="(e: KeyboardEvent) => onRuleRowKeydown(e, rule.id || `draft-${idx}`)"
            >
              <div class="rss-rule-row-main">
                <span class="rss-rule-row-name">{{ rule.name || $t('rss.add-rule') }}</span>
                <span class="rss-rule-row-meta">
                  <span v-if="rule.feed_ids.length === 0">{{ $t('rss.all-feeds') }}</span>
                  <span v-else>{{ rule.feed_ids.length }}</span>
                  <span class="rss-rule-row-dot">·</span>
                  <span>P{{ rule.priority }}</span>
                  <span v-if="rule.stats.match_count > 0" class="rss-rule-row-dot">·</span>
                  <span v-if="rule.stats.match_count > 0">
                    {{ $t('rss.rule-stats', { matched: rule.stats.match_count, downloaded: rule.stats.download_count }) }}
                  </span>
                </span>
              </div>
              <Switch
                :model-value="rule.is_active"
                class="rss-rule-row-switch"
                @click.stop
                @update:model-value="(v: boolean) => toggleActive(rule, v)"
              />
            </li>
          </ul>
        </div>

        <!-- Detail pane -->
        <div v-if="form" class="rss-rules-detail">
          <Tabs v-model="currentTab" class="rss-rules-tabs">
            <TabsList>
              <TabsTrigger value="general">{{ $t('rss.rule-tab-general') }}</TabsTrigger>
              <TabsTrigger value="filters">{{ $t('rss.rule-tab-filters') }}</TabsTrigger>
              <TabsTrigger value="series">{{ $t('rss.rule-tab-series') }}</TabsTrigger>
              <TabsTrigger value="quality">{{ $t('rss.rule-tab-quality') }}</TabsTrigger>
              <TabsTrigger value="output">{{ $t('rss.rule-tab-output') }}</TabsTrigger>
              <TabsTrigger value="test">{{ $t('rss.rule-tab-test') }}</TabsTrigger>
            </TabsList>

            <TabsContent value="general" class="rss-tab-pane">
              <div class="rss-field">
                <label>{{ $t('rss.rule-name') }}</label>
                <Input v-model="form.name" />
              </div>
              <div class="rss-field">
                <label>{{ $t('rss.rule-feed-scope') }}</label>
                <div class="rss-feed-scope">
                  <label v-for="feed in feeds" :key="feed.id" class="rss-feed-checkbox">
                    <Checkbox
                      :model-value="form.feed_ids.includes(feed.id)"
                      @update:model-value="(v: boolean) => toggleFeed(feed.id, v)"
                    />
                    <span>{{ feed.title || feed.url }}</span>
                  </label>
                </div>
              </div>
              <div class="rss-field-row">
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-priority') }}</label>
                  <NumberInput v-model="form.priority" :min="0" />
                </div>
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-mode') }}</label>
                  <select v-model="form.mode" class="rss-select">
                    <option value="any-match">{{ $t('rss.rule-mode-any') }}</option>
                    <option value="best-match">{{ $t('rss.rule-mode-best') }}</option>
                  </select>
                </div>
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-cooldown') }}</label>
                  <NumberInput v-model="form.cooldown_secs" :min="0" />
                </div>
              </div>
              <div class="rss-field-row">
                <label class="rss-toggle">
                  <Switch v-model="form.is_active" />
                  <span>{{ $t('rss.rule-active') }}</span>
                </label>
                <label class="rss-toggle">
                  <Switch v-model="form.auto_download" />
                  <span>{{ $t('rss.rule-auto-download') }}</span>
                </label>
              </div>
            </TabsContent>

            <TabsContent value="filters" class="rss-tab-pane">
              <PatternList
                :title="$t('rss.rule-must')"
                :patterns="form.title_must"
                @update="(v: Pattern[]) => (form.title_must = v)"
              />
              <PatternList
                :title="$t('rss.rule-must-not')"
                :patterns="form.title_must_not"
                @update="(v: Pattern[]) => (form.title_must_not = v)"
              />
              <div class="rss-field-row">
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-min-size') }}</label>
                  <NumberInput
                    :model-value="bytesToMb(form.min_size_bytes)"
                    @update:model-value="(v: number) => (form.min_size_bytes = mbToBytes(v))"
                    :min="0"
                  />
                </div>
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-max-size') }}</label>
                  <NumberInput
                    :model-value="bytesToMb(form.max_size_bytes)"
                    @update:model-value="(v: number) => (form.max_size_bytes = mbToBytes(v))"
                    :min="0"
                  />
                </div>
                <div class="rss-field flex-1">
                  <label>{{ $t('rss.rule-min-seeders') }}</label>
                  <NumberInput
                    :model-value="form.min_seeders ?? 0"
                    @update:model-value="(v: number) => (form.min_seeders = v > 0 ? v : null)"
                    :min="0"
                  />
                </div>
              </div>
            </TabsContent>

            <TabsContent value="series" class="rss-tab-pane">
              <div class="rss-field">
                <label>{{ $t('rss.rule-series-name') }}</label>
                <Input
                  :model-value="form.series_filter ?? ''"
                  @update:model-value="(v: string | number) => (form.series_filter = String(v).trim() || null)"
                />
              </div>
              <div class="rss-field">
                <label>{{ $t('rss.rule-seasons') }}</label>
                <Input
                  :model-value="seasonsText"
                  @update:model-value="(v: string | number) => updateSeasons(String(v))"
                  placeholder="1, 2, 3"
                />
              </div>
              <div class="rss-field">
                <label>{{ $t('rss.rule-episode-selector') }}</label>
                <EpisodeSelectorEditor
                  :selector="form.episodes"
                  @update="(v: EpisodeSelector | null) => (form.episodes = v)"
                />
              </div>
            </TabsContent>

            <TabsContent value="quality" class="rss-tab-pane">
              <ChipList
                :label="$t('rss.rule-quality-prefs')"
                :chips="form.quality_preferences"
                @update="(v: string[]) => (form.quality_preferences = v)"
                :placeholder="QUALITY_PLACEHOLDER"
                reorder
              />
              <ChipList
                :label="$t('rss.rule-quality-required')"
                :chips="form.required_qualities"
                @update="(v: string[]) => (form.required_qualities = v)"
                :placeholder="QUALITY_PLACEHOLDER"
              />
              <ChipList
                :label="$t('rss.rule-quality-forbidden')"
                :chips="form.forbidden_qualities"
                @update="(v: string[]) => (form.forbidden_qualities = v)"
                :placeholder="QUALITY_PLACEHOLDER"
              />
              <label class="rss-toggle">
                <Switch v-model="form.upgrade_existing" />
                <span>{{ $t('rss.rule-upgrade-existing') }}</span>
              </label>
            </TabsContent>

            <TabsContent value="output" class="rss-tab-pane">
              <div class="rss-field">
                <label>{{ $t('rss.rule-download-dir') }}</label>
                <Input
                  :model-value="form.download_dir ?? ''"
                  @update:model-value="(v: string | number) => (form.download_dir = String(v).trim() || null)"
                />
              </div>
              <div class="rss-field">
                <label>{{ $t('rss.rule-filename-template') }}</label>
                <Input
                  :model-value="form.filename_template ?? ''"
                  @update:model-value="(v: string | number) => (form.filename_template = String(v).trim() || null)"
                  placeholder="{series} S{season:02}E{episode:02} [{quality}].{ext}"
                />
              </div>
            </TabsContent>

            <TabsContent value="test" class="rss-tab-pane">
              <Button size="sm" variant="outline" :disabled="dryRunBusy" @click="runDryRun">
                {{ $t('rss.rule-dry-run') }}
              </Button>
              <div v-if="dryRunResults.length === 0" class="rss-dry-empty">
                {{ $t('rss.rule-dry-run-empty') }}
              </div>
              <ul v-else class="rss-dry-list">
                <li
                  v-for="r in dryRunResults"
                  :key="r.item_id"
                  :class="['rss-dry-row', { matched: r.matched }]"
                >
                  <span class="rss-dry-title">{{ r.title }}</span>
                  <span class="rss-dry-score">{{ r.score }}</span>
                  <span class="rss-dry-reason">{{ r.reason }}</span>
                </li>
              </ul>
            </TabsContent>
          </Tabs>

          <div class="rss-rules-detail-footer">
            <Button v-if="form.id" size="sm" variant="ghost" class="text-destructive" @click="removeCurrent">
              <Trash2 :size="14" />
            </Button>
            <div class="flex-1" />
            <Button size="sm" :disabled="!isValid" @click="save">
              {{ $t('rss.save') }}
            </Button>
          </div>
        </div>
        <div v-else class="rss-rules-detail rss-rules-detail-empty">
          {{ $t('rss.add-rule') }}
        </div>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="handleClose">{{ $t('rss.close') }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedImports: Pattern/EpisodeSelector are referenced in <template> type annotations
import type {
	DryRunMatch,
	EpisodeSelector,
	Pattern,
	RssRule,
} from "@shared/types/rss";
import { emptyRule } from "@shared/types/rss";
import { Plus, Trash2 } from "lucide-vue-next";
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
import NumberInput from "@/components/ui/NumberInput.vue";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useRssStore } from "@/store/rss";
import ChipList from "./ChipList.vue";
import EpisodeSelectorEditor from "./EpisodeSelectorEditor.vue";
import PatternList from "./PatternList.vue";

const QUALITY_PLACEHOLDER = "1080p, x265, WEB-DL, ...";

export default {
	name: "mo-rss-rules-dialog",
	components: {
		Button,
		Checkbox,
		ChipList,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		EpisodeSelectorEditor,
		Input,
		NumberInput,
		PatternList,
		Plus,
		Switch,
		Tabs,
		TabsContent,
		TabsList,
		TabsTrigger,
		Trash2,
	},
	props: {
		visible: { type: Boolean, default: false },
	},
	emits: ["close"],
	data() {
		return {
			selectedId: null as string | null,
			form: null as RssRule | null,
			currentTab: "general",
			dryRunResults: [] as DryRunMatch[],
			dryRunBusy: false,
			QUALITY_PLACEHOLDER,
		};
	},
	computed: {
		feeds() {
			return useRssStore().feeds;
		},
		rules() {
			return useRssStore().rules;
		},
		seasonsText(): string {
			return this.form?.seasons?.join(", ") ?? "";
		},
		isValid(): boolean {
			return !!this.form && this.form.name.trim().length > 0;
		},
	},
	watch: {
		visible: {
			immediate: true,
			async handler(val: boolean) {
				if (val) {
					await useRssStore().fetchRules();
					if (this.rules.length > 0) {
						this.selectRule(this.rules[0]?.id ?? null);
					} else {
						this.createDraft();
					}
				}
			},
		},
	},
	methods: {
		createDraft() {
			const draft = emptyRule(this.$t("rss.add-rule"));
			this.form = draft;
			this.selectedId = null;
			this.currentTab = "general";
			this.dryRunResults = [];
		},
		selectRule(id: string | null) {
			if (!id) {
				this.form = null;
				this.selectedId = null;
				return;
			}
			const found = this.rules.find((r) => r.id === id);
			if (!found) {
				return;
			}
			this.selectedId = id;
			this.form = JSON.parse(JSON.stringify(found));
			this.currentTab = "general";
			this.dryRunResults = [];
		},
		async toggleActive(rule: RssRule, value: boolean) {
			await useRssStore().updateRule({ ...rule, is_active: value });
			// Keep the open detail form in sync when the toggled row matches the
			// rule currently being edited; otherwise a subsequent save would
			// clobber the new active state with the stale form copy
			if (this.form && rule.id && this.form.id === rule.id) {
				this.form.is_active = value;
			}
		},
		onRuleRowKeydown(event: KeyboardEvent, id: string) {
			if (event.key === "Enter" || event.key === " ") {
				event.preventDefault();
				this.selectRule(id);
			}
		},
		toggleFeed(feedId: string, value: boolean) {
			if (!this.form) {
				return;
			}
			if (value) {
				if (!this.form.feed_ids.includes(feedId)) {
					this.form.feed_ids = [...this.form.feed_ids, feedId];
				}
			} else {
				this.form.feed_ids = this.form.feed_ids.filter((id) => id !== feedId);
			}
		},
		updateSeasons(text: string) {
			if (!this.form) {
				return;
			}
			const parsed = text
				.split(/[,\s]+/)
				.map((s) => Number.parseInt(s.trim(), 10))
				.filter((n) => Number.isFinite(n) && n > 0);
			this.form.seasons = parsed.length > 0 ? parsed : null;
		},
		bytesToMb(b: number | null): number {
			return b ? Math.round(b / (1024 * 1024)) : 0;
		},
		mbToBytes(mb: number): number | null {
			return mb > 0 ? mb * 1024 * 1024 : null;
		},
		async save() {
			if (!this.form) {
				return;
			}
			const store = useRssStore();
			if (this.form.id) {
				await store.updateRule(this.form);
			} else {
				const created = await store.addRule(this.form);
				this.selectRule(created.id);
			}
		},
		async removeCurrent() {
			if (!this.form?.id) {
				return;
			}
			await useRssStore().removeRule(this.form.id);
			if (this.rules.length > 0) {
				this.selectRule(this.rules[0]?.id ?? null);
			} else {
				this.createDraft();
			}
		},
		async runDryRun() {
			if (!this.form) {
				return;
			}
			this.dryRunBusy = true;
			try {
				this.dryRunResults = await useRssStore().dryRunRule(this.form, 50);
			} finally {
				this.dryRunBusy = false;
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
.rss-rules-dialog {
  max-width: 960px;
  width: 90vw;
}

.rss-rules-layout {
  display: grid;
  grid-template-columns: 240px 1fr;
  gap: 12px;
  height: 480px;
  min-height: 0;
}

.rss-rules-master {
  display: flex;
  flex-direction: column;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  overflow: hidden;
  min-height: 0;
}

.rss-rules-master-toolbar {
  padding: 8px;
  border-bottom: 1px solid hsl(var(--border));
}

.rss-rules-list {
  flex: 1;
  overflow-y: auto;
  list-style: none;
  margin: 0;
  padding: 4px;
}

.rss-rule-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px;
  border-radius: 4px;
  cursor: pointer;
  transition: background 0.1s;
}

.rss-rule-row:hover {
  background: hsl(var(--muted) / 0.5);
}

.rss-rule-row.active {
  background: hsl(var(--accent));
}

.rss-rule-row-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rss-rule-row-name {
  font-size: 13px;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rss-rule-row-meta {
  font-size: 11px;
  color: hsl(var(--muted-foreground));
  display: flex;
  gap: 4px;
  align-items: center;
}

.rss-rule-row-dot {
  opacity: 0.5;
}

.rss-rule-row-switch {
  flex-shrink: 0;
}

.rss-rules-empty {
  text-align: center;
  color: hsl(var(--muted-foreground));
  font-size: 12px;
  padding: 24px 12px;
}

.rss-rules-detail {
  display: flex;
  flex-direction: column;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  overflow: hidden;
  min-height: 0;
}

.rss-rules-detail-empty {
  align-items: center;
  justify-content: center;
  color: hsl(var(--muted-foreground));
  font-size: 13px;
}

.rss-rules-tabs {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.rss-tab-pane {
  flex: 1;
  overflow-y: auto;
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.rss-rules-detail-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid hsl(var(--border));
}

.rss-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.rss-field label {
  font-size: 12px;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
}

.rss-field-row {
  display: flex;
  gap: 8px;
  align-items: flex-end;
}

.rss-feed-scope {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 140px;
  overflow-y: auto;
  padding: 6px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
}

.rss-feed-checkbox {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  cursor: pointer;
}

.rss-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  cursor: pointer;
}

.rss-select {
  height: 32px;
  padding: 0 8px;
  font-size: 13px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  background: transparent;
  color: inherit;
}

.flex-1 {
  flex: 1;
}

.text-destructive {
  color: hsl(var(--destructive));
}

.rss-dry-empty {
  font-size: 12px;
  color: hsl(var(--muted-foreground));
  padding: 12px 0;
}

.rss-dry-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rss-dry-row {
  display: grid;
  grid-template-columns: 1fr 48px 120px;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 4px;
  font-size: 12px;
  background: hsl(var(--muted) / 0.3);
}

.rss-dry-row.matched {
  background: hsl(var(--primary) / 0.1);
}

.rss-dry-title {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rss-dry-score {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.rss-dry-reason {
  color: hsl(var(--muted-foreground));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
