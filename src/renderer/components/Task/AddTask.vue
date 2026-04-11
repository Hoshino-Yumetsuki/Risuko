<template>
  <Dialog :open="visible" @update:open="handleDialogOpenChange">
    <DialogContent
      :show-close-button="false"
      :class="['add-task-dialog', { 'add-task-dialog--advanced-open': showAdvanced }]"
    >
      <DialogHeader class="atd-header">
        <div class="atd-header-left">
          <div class="atd-header-icon">
            <Download :size="16" />
          </div>
          <DialogTitle class="atd-header-title">{{ $t('task.new-task') }}</DialogTitle>
        </div>
        <button
          type="button"
          class="atd-close-btn"
          aria-label="Close"
          :disabled="submitting"
          @click="handleClose"
        >
          <X :size="14" />
        </button>
      </DialogHeader>

      <form ref="taskForm" class="atd-form" @submit.prevent>
        <!-- Source Tabs -->
        <div class="atd-source-tabs">
          <button
            type="button"
            class="atd-source-tab"
            :class="{ 'atd-source-tab--active': type === 'uri' }"
            :disabled="submitting"
            @click="handleTabClick({ props: { name: 'uri' } })"
          >
            <Link2 :size="14" />
            {{ $t('task.uri-task') }}
          </button>
          <button
            type="button"
            class="atd-source-tab"
            :class="{ 'atd-source-tab--active': type === 'torrent' }"
            :disabled="submitting"
            @click="handleTabClick({ props: { name: 'torrent' } })"
          >
            <FileArchive :size="14" />
            {{ $t('task.torrent-task') }}
          </button>
        </div>

        <!-- Source Bodies (stacked) -->
        <div class="atd-source-stack">
          <div class="atd-source-body" :class="{ 'atd-source-body--active': type === 'uri' }">
            <div class="atd-source-inner">
              <Textarea
                ref="uri"
                auto-complete="off"
                rows="4"
                :placeholder="$t('task.uri-task-tips')"
                @paste="handleUriPaste"
                v-model="form.uris"
                class="atd-uri-input resize-none"
              />
            </div>
          </div>
          <div class="atd-source-body" :class="{ 'atd-source-body--active': type === 'torrent' }">
            <div class="atd-source-inner">
              <mo-select-torrent v-on:change="handleTorrentChange" />
            </div>
          </div>
        </div>

        <!-- Core Options -->
        <div class="atd-fields">
          <div class="atd-field atd-field--wide">
            <label class="atd-field-label">{{ $t('task.task-out') }}</label>
            <Input :placeholder="$t('task.task-out-tips')" v-model="form.out" />
          </div>
          <div class="atd-field atd-field--narrow">
            <label class="atd-field-label">{{ $t('task.task-split') }}</label>
            <NumberInput v-model="form.split" :min="1" :max="128" />
          </div>
          <div class="atd-field atd-field--full">
            <label class="atd-field-label">{{ $t('task.task-dir') }}</label>
            <div class="mo-input-group mo-input-group--bordered">
              <span class="mo-input-prepend">
                <mo-history-directory @selected="handleHistoryDirectorySelected" />
              </span>
              <Input
                placeholder=""
                v-model="form.dir"
                :readonly="isMas"
                class="flex-1 shadow-none rounded-none border-none noinput"
              />
              <span class="mo-input-append" v-if="isRenderer">
                <mo-select-directory @selected="handleNativeDirectorySelected" />
              </span>
            </div>
          </div>
        </div>

        <!-- Credential Suggest -->
        <mo-credential-suggest
          v-if="type === 'uri'"
          :uris="form.uris"
          :form="form"
        />

        <!-- Credential Save Prompt -->
        <div v-if="showCredentialSavePrompt" class="credential-save-bar">
          <KeyRound :size="12" />
          <span class="credential-save-text">{{ $t('task.save-credential') }}</span>
          <div class="credential-save-actions">
            <button type="button" class="credential-save-btn" @click="handleSaveCredentialForHost">
              {{ $t('task.save-credential-for-host', { host: lastSubmitHost }) }}
            </button>
            <button type="button" class="credential-save-btn" @click="handleSaveCredentialAsProfile">
              {{ $t('task.save-as-profile') }}
            </button>
            <button type="button" class="credential-save-btn credential-save-btn--dismiss" @click="dismissCredentialSave">
              {{ $t('task.dont-save-credential') }}
            </button>
          </div>
        </div>

        <!-- FTP/SFTP Credentials (shown when FTP/FTPS/SFTP URI detected) -->
        <div v-if="ftpDetected" class="atd-fields">
          <div class="atd-field atd-field--full atd-section-label">
            <KeyRound :size="12" />
            <span>{{ $t('task.ftp-credentials') }}</span>
          </div>
          <div class="atd-field">
            <label class="atd-field-label">{{ $t('task.ftp-username') }}</label>
            <Input :placeholder="sftpDetected ? '' : 'anonymous'" v-model="form.ftpUser" />
          </div>
          <div class="atd-field">
            <label class="atd-field-label">{{ $t('task.ftp-password') }}</label>
            <Input type="password" v-model="form.ftpPasswd" />
          </div>
          <template v-if="sftpDetected">
            <div class="atd-field atd-field--full">
              <label class="atd-field-label">{{ $t('task.ssh-private-key') }}</label>
              <div class="mo-input-group mo-input-group--bordered">
                <Input
                  :placeholder="$t('task.ssh-private-key-path-tips')"
                  v-model="form.sftpPrivateKey"
                  class="flex-1 shadow-none rounded-none border-none"
                />
                <span class="mo-input-append">
                  <ui-button variant="ghost" size="sm" @click.stop="handleSelectKeyFile">
                    <FileKey :size="14" />
                  </ui-button>
                </span>
              </div>
            </div>
            <div class="atd-field atd-field--full">
              <Textarea
                auto-complete="off"
                rows="3"
                :placeholder="$t('task.ssh-private-key-paste-tips')"
                v-model="form.sftpPrivateKeyContent"
                class="resize-none font-mono text-xs"
              />
            </div>
            <div class="atd-field atd-field--full">
              <label class="atd-field-label">{{ $t('task.ssh-key-passphrase') }}</label>
              <Input type="password" :placeholder="$t('task.ssh-key-passphrase-tips')" v-model="form.sftpKeyPassphrase" />
            </div>
          </template>
        </div>

        <!-- Advanced Options -->
        <div class="atd-advanced-wrapper" :class="{ 'atd-advanced-wrapper--open': showAdvanced }">
          <div class="atd-advanced">
            <div class="atd-advanced-grid">
              <div class="atd-field atd-field--full">
                <label class="atd-field-label">{{ $t('task.task-user-agent') }}</label>
                <Textarea
                  auto-complete="off"
                  rows="2"
                  :placeholder="$t('task.task-user-agent')"
                  v-model="form.userAgent"
                  class="resize-none"
                />
              </div>
              <div class="atd-field atd-field--full">
                <label class="atd-field-label">{{ $t('task.task-authorization') }}</label>
                <Textarea
                  auto-complete="off"
                  rows="2"
                  :placeholder="$t('task.task-authorization')"
                  v-model="form.authorization"
                  class="resize-none"
                />
              </div>
              <div class="atd-field">
                <label class="atd-field-label">{{ $t('task.task-referer') }}</label>
                <Textarea
                  auto-complete="off"
                  rows="2"
                  :placeholder="$t('task.task-referer')"
                  v-model="form.referer"
                  class="resize-none"
                />
              </div>
              <div class="atd-field">
                <label class="atd-field-label">{{ $t('task.task-cookie') }}</label>
                <Textarea
                  auto-complete="off"
                  rows="2"
                  :placeholder="$t('task.task-cookie')"
                  v-model="form.cookie"
                  class="resize-none"
                />
              </div>
              <div class="atd-field atd-field--full">
                <label class="atd-field-label">
                  {{ $t('task.task-proxy') }}
                  <a
                    class="atd-field-help"
                    target="_blank"
                    href="https://github.com/agalwood/Motrix/wiki/Proxy"
                    rel="noopener noreferrer"
                  >
                    {{ $t('preferences.proxy-tips') }}
                    <ExternalLink :size="11" />
                  </a>
                </label>
                <Input placeholder="[http://][USER:PASSWORD@]HOST[:PORT]" v-model="form.allProxy" />
              </div>
              <div class="atd-field atd-field--full atd-field--checkbox">
                <ui-checkbox
                  :model-value="!!form.newTaskShowDownloading"
                  @change="onNewTaskShowDownloadingChange"
                >
                  {{ $t('task.navigate-to-downloading') }}
                </ui-checkbox>
              </div>
            </div>
          </div>
        </div>
      </form>

      <DialogFooter class="atd-footer">
        <button
          type="button"
          class="atd-advanced-toggle"
          :class="{ 'atd-advanced-toggle--active': showAdvanced }"
          :disabled="submitting"
          @click="showAdvanced = !showAdvanced"
        >
          <SlidersHorizontal :size="13" />
          {{ $t('task.show-advanced-options') }}
          <ChevronDown
            :size="12"
            class="atd-toggle-chevron"
            :class="{ 'atd-toggle-chevron--open': showAdvanced }"
          />
        </button>
        <div class="atd-footer-actions">
          <ui-button :disabled="submitting" @click="handleCancel">{{ $t('app.cancel') }}</ui-button>
          <ui-button variant="primary" :disabled="submitting" @click="submitForm">
            <Download :size="14" style="margin-right: 6px" />
            {{ submitting ? $t('task.loading-add-task') : $t('app.submit') }}
          </ui-button>
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
	TEMP_DOWNLOAD_SUFFIX,
} from "@shared/constants";
import type { DownloadTask } from "@shared/types/task";
import {
	detectResource,
	getTaskName,
	isEd2kLink,
	isFtpFamily,
	isM3u8Link,
	isSftpLink,
	parseEd2kLink,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { open as tauriOpen } from "@tauri-apps/plugin-dialog";
import { isEmpty } from "lodash";
import {
	ChevronDown,
	Download,
	ExternalLink,
	FileArchive,
	FileKey,
	KeyRound,
	Link2,
	SlidersHorizontal,
	X,
} from "lucide-vue-next";
import api from "@/api";
import SelectDirectory from "@/components/Native/SelectDirectory.vue";
import HistoryDirectory from "@/components/Preference/HistoryDirectory.vue";
import CredentialSuggest from "@/components/Task/CredentialSuggest.vue";
import SelectTorrent from "@/components/Task/SelectTorrent.vue";
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
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import {
	buildTorrentPayload,
	buildUriPayload,
	credentialHasContent,
	extractCredentialFromForm,
	extractHostFromUri,
	extractProtocolFromUri,
	inferOutFromUri,
	initTaskForm,
} from "@/utils/task";

export default {
	name: "mo-add-task",
	components: {
		[HistoryDirectory.name]: HistoryDirectory,
		[SelectDirectory.name]: SelectDirectory,
		[SelectTorrent.name]: SelectTorrent,
		[LoadingOverlay.name]: LoadingOverlay,
		[CredentialSuggest.name]: CredentialSuggest,
		UiButton,
		NumberInput,
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
		DialogFooter,
		Input,
		Textarea,
		X,
		Download,
		Link2,
		FileArchive,
		FileKey,
		KeyRound,
		SlidersHorizontal,
		ChevronDown,
		ExternalLink,
	},
	props: {
		visible: {
			type: Boolean,
			default: false,
		},
		type: {
			type: String,
			default: ADD_TASK_TYPE.URI,
		},
	},
	data() {
		return {
			showAdvanced: false,
			form: {},
			submitting: false,
			showCredentialSavePrompt: false,
			lastSubmitHost: "",
			lastSubmitCredential: {} as Record<string, string>,
		};
	},
	computed: {
		isRenderer: () => is.renderer(),
		isMas: () => is.mas(),
		ftpDetected() {
			const uris = (this.form.uris || "").trim();
			if (!uris) {
				return false;
			}
			const firstUri = uris.split("\n")[0].trim();
			return isFtpFamily(firstUri);
		},
		sftpDetected() {
			const uris = (this.form.uris || "").trim();
			if (!uris) {
				return false;
			}
			const firstUri = uris.split("\n")[0].trim();
			return isSftpLink(firstUri);
		},
	},
	watch: {
		type(current, previous) {
			if (this.visible && previous === ADD_TASK_TYPE.URI) {
				return;
			}

			if (current === ADD_TASK_TYPE.URI) {
				setTimeout(() => {
					this.focusUriInput();
				}, 300);
			}
		},
		visible(current) {
			if (current === true) {
				document.addEventListener("keydown", this.handleHotkey);
				this.handleOpen();
			} else {
				document.removeEventListener("keydown", this.handleHotkey);
			}
		},
	},
	beforeUnmount() {
		document.removeEventListener("keydown", this.handleHotkey);
	},
	methods: {
		focusUriInput() {
			const el = this.$refs.uri?.$el;
			if (el) {
				el.focus();
			}
		},
		handleDialogOpenChange(open) {
			if (!open && !this.submitting) {
				this.handleClose();
			}
		},
		async autofillResourceLink() {
			let content = "";
			try {
				content = await readText();
			} catch (_err) {
				return;
			}

			if (!content || content.length > 4096) {
				return;
			}

			const hasResource = detectResource(content);
			if (!hasResource) {
				return;
			}

			if (isEmpty(this.form.uris)) {
				this.form.uris = content;
			}
		},
		handleOpen() {
			this.showAdvanced = false;
			this.showCredentialSavePrompt = false;
			this.form = initTaskForm({
				app: useAppStore().$state,
				preference: usePreferenceStore().$state,
			});
			setTimeout(() => {
				this.focusUriInput();
			}, 100);

			if (this.type === ADD_TASK_TYPE.URI && isEmpty(this.form.uris)) {
				setTimeout(() => {
					this.autofillResourceLink();
				}, 0);
			}

			setTimeout(() => {
				this.detectSpecialResource(this.form.uris);
			}, 150);
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
		},
		handleHotkey(event) {
			if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
				event.preventDefault();
				this.submitForm();
			}
		},
		handleTabClick(tab) {
			const name = tab?.props?.name || tab?.paneName || tab?.name;
			if (name) {
				useAppStore().changeAddTaskType(name);
			}
		},
		handleUriPaste() {
			this.$nextTick(() => {
				const uris = this.form.uris;
				this.detectSpecialResource(uris);
			});
		},
		detectSpecialResource(uris = "") {
			if (uris.includes("thunder://")) {
				this.$msg.warning({
					message: this.$t("task.thunder-link-tips"),
					duration: 6000,
				});
			}
			const firstUri = uris.trim().split("\n")[0];
			if (isEd2kLink(firstUri)) {
				const parsed = parseEd2kLink(firstUri);
				if (parsed && isEmpty(this.form.out)) {
					this.form.out = parsed.fileName;
				}
				this.$msg.info({
					message: this.$t("task.ed2k-link-detected"),
					duration: 4000,
				});
			} else if (isM3u8Link(firstUri)) {
				this.$msg.info({
					message: this.$t("task.m3u8-link-detected"),
					duration: 4000,
				});
			} else if (isSftpLink(firstUri)) {
				this.$msg.info({
					message: this.$t("task.sftp-link-detected"),
					duration: 4000,
				});
			} else if (isFtpFamily(firstUri)) {
				this.$msg.info({
					message: this.$t("task.ftp-link-detected"),
					duration: 4000,
				});
			}
		},
		async handleSelectKeyFile() {
			const selected = await tauriOpen({
				directory: false,
				multiple: false,
				title: this.$t("task.ssh-private-key"),
			});
			if (selected) {
				this.form.sftpPrivateKey = selected;
			}
		},
		handleTorrentChange(torrentPath = "", selectedFileIndex) {
			const normalizedPath = `${torrentPath || ""}`.trim();
			this.form.torrentPath = normalizedPath;
			if (!normalizedPath) {
				this.form.selectFile = NONE_SELECTED_FILES;
				return;
			}
			this.form.selectFile =
				selectedFileIndex === undefined || selectedFileIndex === null
					? SELECTED_ALL_FILES
					: selectedFileIndex;
		},
		onNewTaskShowDownloadingChange(enable) {
			this.form.newTaskShowDownloading = !!enable;
		},
		handleHistoryDirectorySelected(dir) {
			this.form.dir = dir;
		},
		handleNativeDirectorySelected(dir) {
			this.form.dir = dir;
			usePreferenceStore().recordHistoryDirectory(dir);
		},
		async addTask(type, form) {
			let payload = null;
			if (type === ADD_TASK_TYPE.URI) {
				payload = await buildUriPayload(form);
				await this.resolveTaskNameConflicts(payload, type);
				return useTaskStore().addUri(payload);
			} else if (type === ADD_TASK_TYPE.TORRENT) {
				payload = buildTorrentPayload(form);
				await this.resolveTaskNameConflicts(payload, type);
				return useTaskStore().addTorrent(payload);
			} else {
				logger.error("[Motrix] Add task fail", form);
				throw new Error("task.new-task-unsupported-type");
			}
		},
		normalizePath(value = "") {
			return `${value}`.trim().replace(/[\\/]+$/, "");
		},
		buildConflictKey(dir = "", name = "") {
			const normalizedName = `${name}`.trim();
			if (!normalizedName) {
				return "";
			}
			const normalizedDir = this.normalizePath(dir);
			const key = `${normalizedDir}\u0000${normalizedName}`;
			const platform =
				`${usePreferenceStore().config.platform || ""}`.toLowerCase();
			const isWindows =
				is.windows() || platform === "windows" || platform === "win32";
			return isWindows ? key.toLowerCase() : key;
		},
		normalizeConflictTaskName(task, name = "") {
			const normalized = `${name || ""}`.trim();
			if (!normalized) {
				return "";
			}

			if (task?.bittorrent?.info) {
				return normalized;
			}

			if (normalized.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX)) {
				return normalized.slice(
					0,
					normalized.length - TEMP_DOWNLOAD_SUFFIX.length,
				);
			}

			return normalized;
		},
		resolveConflictName(
			name: string,
			existingKeys: Set<string>,
			targetDir: string,
		): string {
			const key = this.buildConflictKey(targetDir, name);
			if (!key || !existingKeys.has(key)) {
				return name;
			}

			const dotIndex = name.lastIndexOf(".");
			const stem = dotIndex > 0 ? name.slice(0, dotIndex) : name;
			const ext = dotIndex > 0 ? name.slice(dotIndex) : "";

			for (let counter = 1; counter <= 9999; counter++) {
				const candidate = `${stem}.${counter}${ext}`;
				const candidateKey = this.buildConflictKey(targetDir, candidate);
				if (!candidateKey || !existingKeys.has(candidateKey)) {
					return candidate;
				}
			}

			return name;
		},
		async resolveTaskNameConflicts(payload, type) {
			const targetDir = this.normalizePath(
				payload?.options?.dir || this.form?.dir || "",
			);

			// For URI tasks, ensure outs array has inferred names for conflict checking
			if (type === ADD_TASK_TYPE.URI) {
				const uris = payload?.uris || [];
				if (!payload.outs) {
					payload.outs = [];
				}
				for (let i = 0; i < uris.length; i++) {
					if (!payload.outs[i] || !`${payload.outs[i]}`.trim()) {
						payload.outs[i] = await inferOutFromUri(uris[i]);
					}
				}
			}

			let newNames: string[] = [];
			if (type === ADD_TASK_TYPE.URI) {
				newNames = (payload?.outs || []).map((item) => `${item || ""}`.trim());
			} else if (type === ADD_TASK_TYPE.TORRENT) {
				const out = `${payload?.options?.out || ""}`.trim();
				if (out) {
					newNames = [out];
				}
			}

			if (newNames.every((n) => !n)) {
				return;
			}

			const [active, waiting, stopped]: (DownloadTask & { out?: string })[][] =
				await Promise.all([
					api.fetchActiveTaskList(),
					api.fetchWaitingTaskList(),
					api.fetchStoppedTaskList(),
				]);

			const allTasks = [
				...(active || []),
				...(waiting || []),
				...(stopped || []),
			];
			const existing = new Set(
				allTasks
					.map((task) => {
						const existingName = this.normalizeConflictTaskName(
							task,
							task?.out || getTaskName(task, { defaultName: "", maxLen: -1 }),
						);
						return this.buildConflictKey(task?.dir || "", existingName);
					})
					.filter(Boolean),
			);

			if (type === ADD_TASK_TYPE.URI && payload?.outs) {
				for (let i = 0; i < payload.outs.length; i++) {
					const name = `${payload.outs[i] || ""}`.trim();
					if (!name) {
						continue;
					}
					const resolved = this.resolveConflictName(name, existing, targetDir);
					payload.outs[i] = resolved;
					// Track resolved name to prevent self-conflicts within the batch
					const resolvedKey = this.buildConflictKey(targetDir, resolved);
					if (resolvedKey) {
						existing.add(resolvedKey);
					}
				}
			} else if (type === ADD_TASK_TYPE.TORRENT && payload?.options) {
				const name = `${payload.options.out || ""}`.trim();
				if (name) {
					payload.options.out = this.resolveConflictName(
						name,
						existing,
						targetDir,
					);
				}
			}
		},
		async submitForm() {
			if (this.submitting) {
				return;
			}
			this.submitting = true;
			try {
				const formSnapshot = { ...this.form };
				await this.addTask(this.type, this.form);
				this.checkCredentialSave(formSnapshot);
				useAppStore().hideAddTaskDialog();
				if (this.form.newTaskShowDownloading) {
					this.$router
						.push({
							path: "/task/active",
						})
						.catch((err) => {
							logger.log(err);
						});
				}
			} catch (err) {
				const raw = typeof err === "string" ? err : err?.rawMessage;
				const key = typeof err === "string" ? "" : err?.message;
				this.$msg.error(raw || this.$t(key || "task.new-task-fail"));
			} finally {
				this.submitting = false;
			}
		},
		checkCredentialSave(form: Record<string, unknown>) {
			const cred = extractCredentialFromForm(
				form as import("@/utils/task").TaskForm,
			);
			if (!credentialHasContent(cred)) {
				return;
			}
			const host = extractHostFromUri(form.uris as string);
			if (!host) {
				return;
			}

			const store = usePreferenceStore();
			const existing = store.findCredentialsByHost(host);
			const isDuplicate = existing.some((e) => {
				return (
					(e.ftpUser || "") === (cred.ftpUser || "") &&
					(e.ftpPasswd || "") === (cred.ftpPasswd || "") &&
					(e.authorization || "") === (cred.authorization || "") &&
					(e.cookie || "") === (cred.cookie || "") &&
					(e.allProxy || "") === (cred.allProxy || "")
				);
			});
			if (isDuplicate) {
				return;
			}

			this.lastSubmitHost = host;
			this.lastSubmitCredential = {
				...cred,
				host,
				protocol: extractProtocolFromUri(form.uris as string),
			};
			this.showCredentialSavePrompt = true;
		},
		handleSaveCredentialForHost() {
			const now = Date.now();
			const id = `${now}-${Math.random().toString(36).slice(2, 8)}`;
			usePreferenceStore().saveCredential({
				id,
				...this.lastSubmitCredential,
				createdAt: now,
				lastUsedAt: now,
			});
			this.showCredentialSavePrompt = false;
			this.$msg.success({
				message: this.$t("task.credential-saved"),
				duration: 2000,
			});
		},
		handleSaveCredentialAsProfile() {
			const label = window.prompt(
				this.$t("task.credential-profile-name"),
				this.lastSubmitHost,
			);
			if (!label) {
				return;
			}
			const now = Date.now();
			const id = `${now}-${Math.random().toString(36).slice(2, 8)}`;
			usePreferenceStore().saveCredential({
				id,
				label,
				...this.lastSubmitCredential,
				createdAt: now,
				lastUsedAt: now,
			});
			this.showCredentialSavePrompt = false;
			this.$msg.success({
				message: this.$t("task.credential-saved"),
				duration: 2000,
			});
		},
		dismissCredentialSave() {
			this.showCredentialSavePrompt = false;
		},
	},
};
</script>
