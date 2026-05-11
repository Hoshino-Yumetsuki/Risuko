<template>
  <Dialog :open="visible" @update:open="handleDialogOpenChange">
    <DialogContent
      :show-close-button="false"
      class="add-task-dialog flex max-h-[80vh] w-170 flex-col gap-0 overflow-hidden p-0"
    >
      <DialogHeader class="flex shrink-0 flex-row items-center gap-2 border-b border-border/60 px-4 py-3">
        <div class="flex size-7 items-center justify-center rounded-md bg-muted text-muted-foreground">
          <Download :size="14" />
        </div>
        <DialogTitle class="flex-1 text-sm font-medium">{{ $t('task.batch-add-title') }}</DialogTitle>
        <button
          type="button"
          class="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
          aria-label="Close"
          :disabled="submitting"
          @click="handleClose"
        >
          <X :size="14" />
        </button>
      </DialogHeader>

      <div class="atd-scroll min-h-0 flex-1 overflow-y-auto">
        <!-- Drop / Input zone -->
        <div class="flex flex-col gap-2 px-4 pt-3">
          <Textarea
            ref="uri"
            auto-complete="off"
            rows="3"
            :placeholder="$t('task.uri-task-tips')"
            v-model="uriDraft"
            class="resize-none text-xs"
            :disabled="submitting"
            @paste="handleUriPaste"
            @keydown.enter.exact="handleUriEnter"
            @keydown.enter.meta.exact="handleUriSubmitShortcut"
            @keydown.enter.ctrl.exact="handleUriSubmitShortcut"
          />
          <div
            class="flex items-center justify-center gap-2 rounded-md border border-dashed border-border/60 px-3 py-2.5 text-xs text-muted-foreground transition-colors"
            :class="{ 'border-primary/60 bg-primary/5': dragOver }"
            @dragover.prevent
            @dragenter.prevent="dragOver = true"
            @dragleave.prevent="dragOver = false"
            @drop.prevent="handleDrop"
          >
            <FileArchive :size="13" />
            <span>{{ $t('task.batch-drop-hint') }}</span>
            <button
              v-if="isRenderer"
              type="button"
              class="ml-1 rounded-md border border-border/60 px-2 py-0.5 text-[11px] hover:bg-muted"
              :disabled="submitting"
              @click="browseTorrentFiles"
            >
              {{ $t('app.browse') }}
            </button>
          </div>
        </div>

        <!-- Queue zone -->
        <div class="mt-3 mb-12 px-4">
          <div
            v-if="queue.length === 0"
            class="flex min-h-30 items-center justify-center rounded-md border border-dashed border-border/40 text-xs text-muted-foreground"
          >
            {{ $t('task.batch-queue-empty') }}
          </div>
          <Accordion
            v-else
            type="multiple"
            v-model="openItemIds"
            class="flex flex-col gap-2"
          >
            <AnimatePresence>
              <Motion
                v-for="item in queue"
                :key="item.id"
                tag="div"
                layout
                :initial="motionInitial"
                :animate="motionAnimate"
                :exit="motionExit"
                :transition="motionTransition"
              >
                <mo-batch-item-card
                  :item="item"
                  :disabled="submitting"
                  @remove="removeItem"
                  @update:select-file="updateSelection"
                />
              </Motion>
            </AnimatePresence>
          </Accordion>
        </div>

        <!-- Shared options -->
        <div class="border-t border-border/60 px-4 py-3">
          <div class="grid grid-cols-[1fr_88px] gap-2">
            <div>
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-dir') }}</label>
              <div class="mo-input-group mo-input-group--bordered">
                <Input
                  v-model="form.dir"
                  :readonly="isMas"
                  class="flex-1 shadow-none rounded-none border-none noinput"
                />
                <span class="mo-input-append" v-if="isRenderer">
                  <mo-select-directory @selected="handleNativeDirectorySelected" />
                </span>
              </div>
            </div>
            <div>
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-split') }}</label>
              <NumberInput v-model="form.split" :min="1" :max="128" />
            </div>
          </div>
        </div>

        <!-- Advanced options -->
        <AnimatePresence>
          <Motion
            v-if="showAdvanced"
            key="advanced"
            tag="div"
            class="overflow-hidden border-t border-border/60"
            :initial="{ height: 0, opacity: 0 }"
            :animate="{ height: 'auto', opacity: 1 }"
            :exit="{ height: 0, opacity: 0 }"
            :transition="{ duration: reducedMotion ? 0 : 0.22, ease: [0.22, 0.61, 0.36, 1] }"
          >
            <div class="grid grid-cols-2 gap-x-3 gap-y-3 px-4 py-3">
            <div class="col-span-2">
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-user-agent') }}</label>
              <Textarea
                auto-complete="off"
                rows="2"
                :placeholder="$t('task.task-user-agent')"
                v-model="form.userAgent"
                class="resize-none text-xs"
              />
            </div>
            <div class="col-span-2">
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-authorization') }}</label>
              <Textarea
                auto-complete="off"
                rows="2"
                :placeholder="$t('task.task-authorization')"
                v-model="form.authorization"
                class="resize-none text-xs"
              />
            </div>
            <div>
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-referer') }}</label>
              <Textarea
                auto-complete="off"
                rows="2"
                :placeholder="$t('task.task-referer')"
                v-model="form.referer"
                class="resize-none text-xs"
              />
            </div>
            <div>
              <label class="mb-1 block text-[11px] text-muted-foreground">{{ $t('task.task-cookie') }}</label>
              <Textarea
                auto-complete="off"
                rows="2"
                :placeholder="$t('task.task-cookie')"
                v-model="form.cookie"
                class="resize-none text-xs"
              />
            </div>
            <div class="col-span-2">
              <label class="mb-1 flex items-center gap-1 text-[11px] text-muted-foreground">
                {{ $t('task.task-proxy') }}
                <a
                  class="inline-flex items-center gap-0.5 text-[10px] text-primary hover:underline"
                  target="_blank"
                  href="https://github.com/YueMiyuki/Risuko/wiki/Proxy"
                  rel="noopener noreferrer"
                >
                  {{ $t('preferences.proxy-tips') }}
                  <ExternalLink :size="10" />
                </a>
              </label>
              <Input
                placeholder="[http://][USER:PASSWORD@]HOST[:PORT]"
                v-model="form.allProxy"
              />
            </div>
            <div class="col-span-2">
              <ui-checkbox
                :model-value="!!form.newTaskShowDownloading"
                @change="onNewTaskShowDownloadingChange"
              >
                {{ $t('task.navigate-to-downloading') }}
              </ui-checkbox>
            </div>
            <div class="col-span-2">
              <ui-checkbox
                :model-value="!!form.completionScriptOverride"
                @change="(v) => (form.completionScriptOverride = !!v)"
              >
                {{ $t('task.completion-script-override') }}
              </ui-checkbox>
            </div>
            <template v-if="form.completionScriptOverride">
              <div class="col-span-2">
                <label class="mb-1 block text-[11px] text-muted-foreground">{{
                  $t('preferences.completion-script-command')
                }}</label>
                <Input
                  v-model="form.completionScriptCommand"
                  :placeholder="
                    $t('preferences.completion-script-command-placeholder')
                  "
                />
              </div>
              <div class="col-span-2">
                <label class="mb-1 block text-[11px] text-muted-foreground">{{
                  $t('preferences.completion-script-args')
                }}</label>
                <Input
                  v-model="form.completionScriptArgs"
                  :placeholder="
                    $t('preferences.completion-script-args-placeholder')
                  "
                />
              </div>
              <div>
                <label class="mb-1 block text-[11px] text-muted-foreground">{{
                  $t('preferences.completion-script-timeout')
                }}</label>
                <NumberInput
                  v-model="form.completionScriptTimeoutMs"
                  :min="1000"
                  :max="300000"
                  :step="1000"
                />
              </div>
            </template>
            </div>
          </Motion>
        </AnimatePresence>
      </div>

      <DialogFooter class="flex shrink-0 flex-row items-center justify-between border-t border-border/60 px-4 py-3 sm:justify-between">
        <button
          type="button"
          class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
          :disabled="submitting"
          @click="showAdvanced = !showAdvanced"
        >
          <SlidersHorizontal :size="12" />
          {{ $t('task.show-advanced-options') }}
          <ChevronDown
            :size="12"
            class="transition-transform"
            :class="{ 'rotate-180': showAdvanced }"
          />
        </button>
        <div class="flex items-center gap-2">
          <ui-button :disabled="submitting" @click="handleCancel">{{ $t('app.cancel') }}</ui-button>
          <Motion :while-tap="reducedMotion ? undefined : { scale: 0.97 }" tag="div">
            <ui-button
              variant="primary"
              :disabled="submitting || !canSubmit"
              @click="submitForm"
            >
              <Download :size="14" style="margin-right: 6px" />
              {{ submitting ? $t('task.loading-add-task') : $t('task.batch-submit', { count: submitCount }) }}
            </ui-button>
          </Motion>
        </div>
      </DialogFooter>
      <mo-loading-overlay :show="submitting" :text="$t('task.loading-add-task')" />
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import {
	ADD_TASK_TYPE,
	NONE_SELECTED_FILES,
	SELECTED_ALL_FILES,
} from "@shared/constants";
import { detectUriProtocol, splitTaskLinks } from "@shared/utils";
import logger from "@shared/utils/logger";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { open as tauriOpen } from "@tauri-apps/plugin-dialog";
import {
	ChevronDown,
	Download,
	ExternalLink,
	FileArchive,
	SlidersHorizontal,
	X,
} from "lucide-vue-next";
import { AnimatePresence, Motion } from "motion-v";
import { toast } from "vue-sonner";
import SelectDirectory from "@/components/Native/SelectDirectory.vue";
import BatchItemCard from "@/components/Task/BatchItemCard.vue";
import { Accordion } from "@/components/ui/accordion";
import UiButton from "@/components/ui/compat/UiButton.vue";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import LoadingOverlay from "@/components/ui/LoadingOverlay.vue";
import NumberInput from "@/components/ui/NumberInput.vue";
import { Textarea } from "@/components/ui/textarea";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import {
	type BatchQueueItem,
	createTorrentBatchItem,
	createUriBatchItem,
} from "@/store/batchQueue";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import { buildOption, initTaskForm, type TaskForm } from "@/utils/task";

interface FileLikeWithPath {
	name: string;
	path?: string;
}

export default {
	name: "mo-add-task",
	components: {
		[BatchItemCard.name]: BatchItemCard,
		[SelectDirectory.name]: SelectDirectory,
		[LoadingOverlay.name]: LoadingOverlay,
		UiButton,
		NumberInput,
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
		DialogFooter,
		Input,
		Textarea,
		Accordion,
		AnimatePresence,
		Motion,
		Download,
		FileArchive,
		SlidersHorizontal,
		ChevronDown,
		ExternalLink,
		X,
	},
	props: {
		visible: { type: Boolean, default: false },
		type: { type: String, default: ADD_TASK_TYPE.URI },
	},
	data() {
		return {
			form: {} as TaskForm,
			uriDraft: "",
			submitting: false,
			dragOver: false,
			showAdvanced: false,
			openItemIds: [] as string[],
			reducedMotion: false,
			lastQueueLength: 0,
		};
	},
	computed: {
		isRenderer: () => is.renderer(),
		isMas: () => is.mas(),
		queue(): BatchQueueItem[] {
			return useAppStore().addTaskQueue;
		},
		motionInitial() {
			return this.reducedMotion ? false : { opacity: 0, y: 8, scale: 0.98 };
		},
		motionAnimate() {
			return this.reducedMotion
				? { opacity: 1 }
				: { opacity: 1, y: 0, scale: 1 };
		},
		motionExit() {
			return this.reducedMotion
				? { opacity: 0 }
				: { opacity: 0, x: -24, height: 0 };
		},
		motionTransition() {
			return this.reducedMotion
				? { duration: 0 }
				: { type: "spring", stiffness: 360, damping: 30 };
		},
		draftLinkCount(): number {
			return splitTaskLinks(this.uriDraft || "").length;
		},
		canSubmit(): boolean {
			return this.queue.length > 0 || this.draftLinkCount > 0;
		},
		submitCount(): number {
			return this.queue.length + this.draftLinkCount;
		},
	},
	watch: {
		visible(current) {
			if (current) {
				this.handleOpen();
			}
		},
		queue: {
			deep: false,
			handler(items: BatchQueueItem[]) {
				if (items.length > this.lastQueueLength) {
					const newOnes = items.slice(this.lastQueueLength);
					const ids = new Set(this.openItemIds);
					for (const it of newOnes) {
						ids.add(it.id);
					}
					this.openItemIds = Array.from(ids);
				}
				this.lastQueueLength = items.length;
			},
		},
	},
	mounted() {
		if (typeof window !== "undefined" && window.matchMedia) {
			const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
			this.reducedMotion = mq.matches;
			const handler = (e: MediaQueryListEvent) => {
				this.reducedMotion = e.matches;
			};
			this._reducedMotionMq = mq;
			this._reducedMotionHandler = handler;
			if (typeof mq.addEventListener === "function") {
				mq.addEventListener("change", handler);
			} else if (
				typeof (
					mq as MediaQueryList & {
						addListener?: (l: (e: MediaQueryListEvent) => void) => void;
					}
				).addListener === "function"
			) {
				(
					mq as MediaQueryList & {
						addListener: (l: (e: MediaQueryListEvent) => void) => void;
					}
				).addListener(handler);
			}
		}
		if (this.visible) {
			this.handleOpen();
		}
	},
	beforeUnmount() {
		const mq = this._reducedMotionMq;
		const handler = this._reducedMotionHandler;
		if (mq && handler) {
			if (typeof mq.removeEventListener === "function") {
				mq.removeEventListener("change", handler);
			} else if (
				typeof (
					mq as MediaQueryList & {
						removeListener?: (l: (e: MediaQueryListEvent) => void) => void;
					}
				).removeListener === "function"
			) {
				(
					mq as MediaQueryList & {
						removeListener: (l: (e: MediaQueryListEvent) => void) => void;
					}
				).removeListener(handler);
			}
		}
		this._reducedMotionMq = null;
		this._reducedMotionHandler = null;
	},
	methods: {
		handleOpen() {
			this.showAdvanced = false;
			this.form = initTaskForm({
				app: useAppStore().$state,
				preference: usePreferenceStore().$state,
			});
			// pull pre-seeded torrents (drag-drop or commands)
			const seeded = useAppStore().addTaskTorrents;
			if (seeded.length > 0) {
				const items = seeded
					.filter((s) => /\.torrent$/i.test(`${s.path || ""}`))
					.map((s) => createTorrentBatchItem(s.path, s.name));
				if (items.length > 0) {
					useAppStore().enqueueBatchItems(items);
				}
				useAppStore().addTaskAddTorrents({ fileList: [] });
			}
			// pre-seeded uri
			const seededUri = useAppStore().addTaskUrl;
			let hasSeededUri = false;
			if (seededUri?.trim()) {
				hasSeededUri = true;
				this.uriDraft = seededUri;
				this.commitUriDraft();
				useAppStore().updateAddTaskUrl("");
			}
			this.lastQueueLength = useAppStore().addTaskQueue.length;
			this.openItemIds = useAppStore().addTaskQueue.map((it) => it.id);
			// Auto fill from clipboard when nothing else seeded the input.
			if (!hasSeededUri && !(this.uriDraft || "").trim()) {
				this.tryFillFromClipboard();
			}
			// Focus the URI textarea for immediate typing/pasting.
			this.$nextTick(() => {
				this.focusUriInput();
			});
		},
		async tryFillFromClipboard() {
			let text = "";
			try {
				// Race clipboard read against a timeout to prevent GTK main-thread
				// deadlock on Linux (WebKitGTK clipboard IPC can block indefinitely
				// if fired while the dialog portal is still mounting).
				text = await Promise.race([
					readText(),
					new Promise<string>((_resolve, reject) =>
						setTimeout(() => reject(new Error("clipboard timeout")), 1000),
					),
				]);
			} catch (err) {
				logger.warn("[Risuko] readText clipboard failed:", err);
				return;
			}
			if (!text || text.length > 4096) {
				return;
			}
			const firstLine = text.split(/\r?\n/)[0]?.trim() || "";
			if (!firstLine || !isLikelyTaskLink(firstLine)) {
				return;
			}
			// Don't clobber if user already typed something between open and read.
			if ((this.uriDraft || "").trim()) {
				return;
			}
			this.uriDraft = text.trim();
		},
		focusUriInput() {
			const ref = this.$refs.uri as { $el?: HTMLElement } | undefined;
			const el = ref?.$el;
			if (el && typeof (el as HTMLTextAreaElement).focus === "function") {
				(el as HTMLTextAreaElement).focus();
			}
		},
		handleDialogOpenChange(open: boolean) {
			if (!open && !this.submitting) {
				this.handleClose();
			}
		},
		handleCancel() {
			if (this.submitting) {
				return;
			}
			useAppStore().hideAddTaskDialog();
		},
		handleClose() {
			if (this.submitting) {
				return;
			}
			const appStore = useAppStore();
			appStore.hideAddTaskDialog();
			appStore.updateAddTaskOptions({});
			this.uriDraft = "";
		},
		handleUriPaste() {
			this.$nextTick(() => {
				this.commitUriDraft();
			});
		},
		handleUriEnter(ev: KeyboardEvent) {
			if (ev.shiftKey) {
				return;
			}
			ev.preventDefault();
			this.commitUriDraft();
		},
		handleUriSubmitShortcut(ev: KeyboardEvent) {
			ev.preventDefault();
			if (this.submitting || !this.canSubmit) {
				return;
			}
			this.submitForm();
		},
		commitUriDraft() {
			const text = this.uriDraft || "";
			const links = splitTaskLinks(text);
			if (links.length === 0) {
				return;
			}
			const items = links.map((u) => createUriBatchItem(u));
			useAppStore().enqueueBatchItems(items);
			this.notifyDetectedProtocols(links);
			this.uriDraft = "";
		},
		notifyDetectedProtocols(links: string[]) {
			if (!links || links.length === 0) {
				return;
			}
			const counts = new Map<string, number>();
			let unknown = 0;
			for (const uri of links) {
				const proto = detectUriProtocol(uri);
				if (proto) {
					counts.set(proto, (counts.get(proto) || 0) + 1);
				} else {
					unknown += 1;
				}
			}
			if (counts.size === 0 && unknown === 0) {
				return;
			}
			const parts = Array.from(counts.entries()).map(([name, n]) =>
				n > 1 ? `${name} \u00d7${n}` : name,
			);
			if (unknown > 0) {
				parts.push(
					unknown > 1
						? this.$t("task.unknown-protocol-multi", { count: unknown })
						: this.$t("task.unknown-protocol"),
				);
			}
			const summary = parts.join(", ");
			const title =
				counts.size === 1 && unknown === 0 && links.length === 1
					? this.$t("task.link-detected-single", { name: parts[0] })
					: this.$t("task.links-detected");
			toast.info(title, { description: summary });
		},
		handleDrop(ev: DragEvent) {
			this.dragOver = false;
			const files = Array.from(
				ev.dataTransfer?.files || [],
			) as FileLikeWithPath[];
			this.acceptTorrentFiles(files);
		},
		acceptTorrentFiles(files: FileLikeWithPath[]) {
			const items: BatchQueueItem[] = [];
			for (const f of files) {
				const path = `${f.path || ""}`.trim();
				if (!path || !/\.torrent$/i.test(path)) {
					continue;
				}
				items.push(createTorrentBatchItem(path, f.name));
			}
			if (items.length === 0) {
				toast.error(this.$t("task.new-task-torrent-required"));
				return;
			}
			useAppStore().enqueueBatchItems(items);
			toast.info(
				items.length === 1
					? this.$t("task.bittorrent-detected")
					: this.$t("task.bittorrent-detected-multi", {
							count: items.length,
						}),
			);
		},
		async browseTorrentFiles() {
			try {
				const selected = await tauriOpen({
					multiple: true,
					filters: [{ name: "Torrent", extensions: ["torrent"] }],
				});
				if (!selected) {
					return;
				}
				// `multiple: true` is documented to return string[] | null, but be
				// defensive: a single string would also be a valid path here.
				const arr = Array.isArray(selected) ? selected : [selected];
				const items: BatchQueueItem[] = [];
				for (const path of arr) {
					if (typeof path !== "string" || !path) {
						continue;
					}
					const name = segmentName(path);
					items.push(createTorrentBatchItem(path, name));
				}
				if (items.length > 0) {
					useAppStore().enqueueBatchItems(items);
				}
			} catch (err) {
				logger.warn("[Risuko] browseTorrentFiles failed:", err);
			}
		},
		removeItem(id: string) {
			useAppStore().removeBatchItem(id);
			this.openItemIds = this.openItemIds.filter((v) => v !== id);
		},
		updateSelection(id: string, selectFile: string) {
			useAppStore().updateBatchItem(id, { selectFile });
		},
		handleNativeDirectorySelected(dir: string) {
			this.form.dir = dir;
			usePreferenceStore().recordHistoryDirectory(dir);
		},
		onNewTaskShowDownloadingChange(enable: boolean) {
			this.form.newTaskShowDownloading = !!enable;
		},
		buildSharedOptions(extra: Record<string, unknown> = {}) {
			const base = buildOption(ADD_TASK_TYPE.TORRENT, this.form);
			// strip selectFile / out from shared (per-item)
			delete (base as Record<string, unknown>).selectFile;
			delete (base as Record<string, unknown>).out;
			return { ...base, ...extra };
		},
		normalizeAddTaskError(err: unknown): string {
			// Tauri rejects with a string payload (typically an i18n key).
			// Engine-thrown errors may carry a `rawMessage` and i18n `message`.
			if (typeof err === "string") {
				return err.startsWith("task.") ? this.$t(err) : err;
			}
			const e = err as { rawMessage?: string; message?: string } | null;
			const raw = e?.rawMessage;
			if (raw) {
				return raw;
			}
			const key = e?.message;
			if (key) {
				return key.startsWith("task.") ? this.$t(key) : key;
			}
			return this.$t("task.batch-all-failed");
		},
		async submitForm() {
			if (this.submitting) {
				return;
			}
			const appStore = useAppStore();
			// Commit any pending draft URI(s) so users can click Add directly
			// after pasting/typing without first hitting Enter.
			if ((this.uriDraft || "").trim()) {
				this.commitUriDraft();
			}
			const items = appStore.addTaskQueue;
			if (items.length === 0) {
				return;
			}
			this.submitting = true;

			const torrentItems = items.filter(
				(it) => it.kind === "torrent" && it.path,
			);
			const invalidTorrentItems = items.filter(
				(it) => it.kind === "torrent" && !it.path,
			);
			const uriItems = items.filter(
				(it) => it.kind === "uri" || it.kind === "magnet",
			);

			const taskStore = useTaskStore();
			let okCount = 0;
			let failCount = 0;

			// Mark all as submitting
			for (const it of items) {
				appStore.updateBatchItem(it.id, { status: "submitting", error: "" });
			}

			// Short-circuit torrent items missing a resolvable path so they don't
			// stay in the "submitting" state and are reflected in fail totals.
			if (invalidTorrentItems.length > 0) {
				const invalidMsg = this.$t("task.new-task-torrent-required");
				for (const it of invalidTorrentItems) {
					appStore.updateBatchItem(it.id, {
						status: "failed",
						error: invalidMsg,
					});
					failCount += 1;
				}
			}

			// Torrents: batch
			if (torrentItems.length > 0) {
				try {
					const sharedOpts = this.buildSharedOptions();
					const results = await taskStore.addTorrents({
						paths: torrentItems.map((it) => it.path as string),
						options: sharedOpts,
					});
					for (const res of results) {
						const item = torrentItems.find((it) => it.path === res.path);
						if (!item) {
							continue;
						}
						if (res.gid) {
							okCount += 1;
							appStore.updateBatchItem(item.id, {
								status: "success",
								gid: res.gid,
							});
						} else {
							failCount += 1;
							appStore.updateBatchItem(item.id, {
								status: "failed",
								error: res.error || this.$t("task.batch-all-failed"),
							});
						}
					}
				} catch (err: unknown) {
					failCount += torrentItems.length;
					const msg = this.normalizeAddTaskError(err);
					for (const it of torrentItems) {
						appStore.updateBatchItem(it.id, {
							status: "failed",
							error: msg,
						});
					}
				}
			}

			// URIs / magnets: per-item submission so a single failure doesn't
			// poison the whole batch and so per-item filenames can be inferred.
			for (const it of uriItems) {
				try {
					const uri = it.uri as string;
					const sharedOpts = this.buildSharedOptions();
					const sel = it.selectFile;
					if (
						sel &&
						sel !== SELECTED_ALL_FILES &&
						sel !== NONE_SELECTED_FILES
					) {
						sharedOpts.selectFile = sel;
					}
					const outs = it.out ? [it.out] : [];
					await taskStore.addUri({
						uris: [uri],
						outs,
						options: sharedOpts as Record<string, string>,
					});
					okCount += 1;
					appStore.updateBatchItem(it.id, { status: "success" });
				} catch (err: unknown) {
					failCount += 1;
					const msg = this.normalizeAddTaskError(err);
					appStore.updateBatchItem(it.id, {
						status: "failed",
						error: msg,
					});
				}
			}

			this.submitting = false;
			const total = items.length;

			if (failCount === 0) {
				toast.success(this.$t("task.batch-all-success", { count: okCount }));
				appStore.hideAddTaskDialog();
				if (this.form.newTaskShowDownloading) {
					this.$router
						.push({ path: "/task/active" })
						.catch((err: unknown) => logger.log(err));
				}
			} else if (okCount === 0) {
				toast.error(this.$t("task.batch-all-failed"));
			} else {
				toast.warning(
					this.$t("task.batch-partial-success", { ok: okCount, total }),
				);
				// Hide successes via exit animation
				const remaining = appStore.addTaskQueue.filter(
					(it) => it.status !== "success",
				);
				appStore.resetBatchQueue();
				appStore.enqueueBatchItems(remaining);
			}
		},
	},
};

function segmentName(path: string): string {
	const segs = `${path}`.split(/[/\\]/);
	return segs[segs.length - 1] || path;
}

function isLikelyTaskLink(line: string): boolean {
	const lower = line.toLowerCase();
	return (
		lower.startsWith("http://") ||
		lower.startsWith("https://") ||
		lower.startsWith("ftp://") ||
		lower.startsWith("ftps://") ||
		lower.startsWith("sftp://") ||
		lower.startsWith("magnet:") ||
		lower.startsWith("thunder://") ||
		lower.startsWith("ed2k://") ||
		lower.startsWith("adc://") ||
		lower.startsWith("adcs://") ||
		lower.startsWith("dchub://") ||
		lower.startsWith("nmdc://") ||
		lower.startsWith("gnutella://") ||
		lower.startsWith("gnet://") ||
		lower.startsWith("g2://") ||
		lower.startsWith("gift://")
	);
}
</script>

<style scoped>
.add-task-dialog :deep(.upload-torrent) {
  display: none;
}
.atd-scroll {
  scrollbar-width: none;
  -ms-overflow-style: none;
}
.atd-scroll::-webkit-scrollbar {
  width: 0;
  height: 0;
  display: none;
}
</style>
