<template>
  <div class="content panel panel-layout panel-layout--v share-page">
    <mo-enter tag="header" preset="fadeInDown" class="panel-header">
      <h4 class="share-title">{{ $t('share.title') }}</h4>
    </mo-enter>

    <main class="panel-content">
      <div class="share-body">
        <section v-if="!isLoggedIn" class="settings-section share-login-required">
          <div class="share-login-card">
            <div class="share-login-icon"><LogIn :size="26" /></div>
            <h3 class="share-login-title">{{ $t('share.login-required-title') }}</h3>
            <p class="share-login-desc">{{ $t('share.login-required-description') }}</p>
            <Button class="share-login-btn" @click="showLoginModal = true">
              <LogIn :size="16" />
              {{ $t('sync.login') }}
            </Button>
          </div>
        </section>

        <template v-else>
        <Tabs :model-value="store.role" class="share-tabs" @update:model-value="onRoleChange">
          <TabsList class="share-tabs-list" :aria-label="$t('share.mode')">
            <TabsTrigger value="send">
              <Upload :size="16" />
              {{ $t('share.send') }}
            </TabsTrigger>
            <TabsTrigger value="receive">
              <Download :size="16" />
              {{ $t('share.receive') }}
            </TabsTrigger>
          </TabsList>

          <div class="share-tab-stage">
            <AnimatePresence mode="wait">
              <Motion
                v-if="store.role === 'send'"
                key="share-send"
                tag="div"
                class="share-tab-panel"
                :initial="panelInitial('send')"
                :animate="panelAnimate"
                :exit="panelExit('send')"
                :transition="panelTransition"
              >
            <section v-if="!activeSend" class="settings-section">
              <div class="settings-section-header">
                <div class="section-icon"><Upload :size="16" /></div>
                <div class="section-title">
                  <h3>{{ $t('share.send-title') }}</h3>
                  <p class="section-desc">{{ $t('share.send-description') }}</p>
                </div>
              </div>
              <div class="settings-section-content">
                <div class="share-picker" :class="{ 'share-picker--empty': !selected.length }">
                  <template v-if="!selected.length">
                    <FolderPlus :size="28" class="share-picker-icon" />
                    <p class="share-picker-hint">{{ $t('share.empty-hint') }}</p>
                  </template>
                  <ul v-else class="share-staged-list" :aria-label="$t('share.selected-files')">
                    <li v-for="(item, idx) in selected" :key="item.path">
                      <Folder v-if="item.isDir" :size="16" class="share-staged-icon" />
                      <File v-else :size="16" class="share-staged-icon" />
                      <span class="share-staged-name">{{ baseName(item.path) }}</span>
                      <button
                        type="button"
                        class="share-staged-remove"
                        :aria-label="$t('share.remove')"
                        @click="removeSelected(idx)"
                      >
                        <X :size="15" />
                      </button>
                    </li>
                  </ul>
                  <div class="share-picker-actions">
                    <Button variant="outline" size="sm" class="share-pick-btn" @click="onAddFiles">
                      <FilePlus2 :size="15" />
                      {{ $t('share.add-files') }}
                    </Button>
                    <Button variant="outline" size="sm" class="share-pick-btn" @click="onAddFolder">
                      <FolderPlus :size="15" />
                      {{ $t('share.add-folder') }}
                    </Button>
                  </div>
                </div>

                <div class="share-cta-row">
                  <span v-if="selected.length" class="share-selected-count">
                    {{ $t('share.selected-count', { count: selected.length }) }}
                  </span>
                  <Button
                    class="share-primary-action"
                    :disabled="!selected.length || preparing"
                    @click="onShare"
                  >
                    <Loader2 v-if="preparing" :size="16" class="animate-spin" />
                    <Share2 v-else :size="16" />
                    {{ preparing ? $t('share.preparing') : $t('share.generate-code') }}
                  </Button>
                </div>
              </div>
            </section>

            <section v-else class="share-result-card" aria-live="polite">
              <div class="share-result-head">
                <div class="share-result-status">
                  <component
                    :is="statusIcon(activeSend)"
                    :size="18"
                    class="share-result-status-icon"
                    :class="{ 'animate-spin': ['preparing', 'connecting'].includes(activeSend.status) }"
                  />
                  <div class="share-result-status-text">
                    <strong>{{ statusLabel(activeSend) }}</strong>
                    <span class="share-result-sub">{{ shareHint(activeSend) }}</span>
                  </div>
                </div>
                <div v-if="showBadge(activeSend)" class="share-path-row">
                  <span
                    class="share-badge"
                    :class="`share-badge--${activeSend.path}`"
                    :title="pathLabel(activeSend.path, false)"
                  >
                    <Zap v-if="activeSend.path === 'direct' || activeSend.path === 'mixed'" :size="12" />
                    <Radio v-else :size="12" />
                    <span class="share-badge-label">{{ pathLabel(activeSend.path) }}</span>
                  </span>
                </div>
              </div>

              <template v-if="activeSend.deviceCode">
                <div v-if="qrDataUrl" class="share-qr">
                  <img :src="qrDataUrl" :alt="$t('share.qr-alt')" width="180" height="180" />
                  <p class="share-qr-hint">{{ $t('share.qr-hint') }}</p>
                </div>

                <p class="share-code-label">{{ $t('share.device-code') }}</p>
                <div class="share-code-row">
                  <span class="share-code" :aria-label="$t('share.device-code')">{{ activeSend.deviceCode }}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="share-copy-btn"
                    :aria-label="$t('share.copy-code')"
                    @click="copy(activeSend.deviceCode || '')"
                  >
                    <Copy :size="16" />
                  </Button>
                </div>

                <p class="share-code-label">{{ $t('share.magic-url') }}</p>
                <div class="share-url-row">
                  <Input :model-value="activeSendUrl" readonly class="share-url-input" :aria-label="$t('share.magic-url')" />
                  <Button
                    variant="ghost"
                    size="sm"
                    class="share-copy-btn"
                    :aria-label="$t('share.copy-url')"
                    @click="copy(activeSendUrl)"
                  >
                    <Copy :size="16" />
                  </Button>
                </div>
              </template>

              <Progress
                v-if="activeSend.status === 'transferring' && activeSend.total > 0"
                :model-value="percent(activeSend)"
                class="share-progress"
              />
              <p v-if="activeSend.status === 'transferring' && activeSend.total > 0" class="share-result-bytes">
                {{ transferProgressText(activeSend) }}
              </p>

              <div class="share-result-actions">
                <Button variant="outline" size="sm" @click="onTransferAction(activeSend)">
                  {{ $t('share.cancel') }}
                </Button>
              </div>
            </section>
              </Motion>
              <Motion
                v-else
                key="share-receive"
                tag="div"
                class="share-tab-panel"
                :initial="panelInitial('receive')"
                :animate="panelAnimate"
                :exit="panelExit('receive')"
                :transition="panelTransition"
              >
            <section class="settings-section">
              <div class="settings-section-header">
                <div class="section-icon"><Download :size="16" /></div>
                <div class="section-title">
                  <h3>{{ $t('share.receive-title') }}</h3>
                  <p class="section-desc">{{ $t('share.receive-description') }}</p>
                </div>
              </div>
              <div class="settings-section-content">
                <form class="share-code-entry" @submit.prevent="onLookup">
                  <Input
                    v-model="codeInput"
                    class="share-code-field"
                    :placeholder="$t('share.code-placeholder')"
                    :aria-label="$t('share.device-code')"
                    autocapitalize="characters"
                    autocomplete="off"
                  />
                  <div class="share-code-actions">
                    <Button
                      v-if="isAndroid"
                      type="button"
                      variant="outline"
                      class="share-scan-btn"
                      :disabled="store.resolving || scanning"
                      :aria-label="$t('share.scan-qr')"
                      @click="onScanQr"
                    >
                      <Loader2 v-if="scanning" :size="16" class="animate-spin" />
                      <ScanLine v-else :size="16" />
                      {{ $t('share.scan-qr') }}
                    </Button>
                    <Button type="submit" class="share-lookup-btn" :disabled="store.resolving || !codeInput.trim()">
                      <Loader2 v-if="store.resolving" :size="16" class="animate-spin" />
                      <Search v-else :size="16" />
                      {{ $t('share.look-up') }}
                    </Button>
                  </div>
                </form>

                <p v-if="store.errorMessage" class="share-error" role="alert">
                  {{ store.errorMessage }}
                </p>

                <div v-if="store.pendingIncoming" class="share-incoming">
                  <p class="share-incoming-title">{{ $t('share.incoming-files') }}</p>
                  <p v-if="!store.pendingIncoming.files.length" class="section-desc">
                    {{ $t('share.incoming-pending') }}
                  </p>
                  <ul v-else class="share-file-list">
                    <li v-for="(file, idx) in store.pendingIncoming.files" :key="idx">
                      <File :size="14" />
                      <span class="share-file-name">{{ file.name }}</span>
                      <span class="share-file-size">{{ formatBytes(file.size) }}</span>
                    </li>
                  </ul>
                  <div class="share-incoming-actions">
                    <Button variant="outline" size="sm" @click="store.clearPending()">
                      {{ $t('share.cancel') }}
                    </Button>
                    <Button size="sm" :disabled="receiving" @click="onReceive">
                      <Loader2 v-if="receiving" :size="16" class="animate-spin" />
                      <FolderDown v-else :size="16" />
                      {{ $t('share.choose-folder-receive') }}
                    </Button>
                  </div>
                </div>
              </div>
            </section>
              </Motion>
            </AnimatePresence>
          </div>
        </Tabs>

        <!-- TRANSFERS -->
        <section v-if="otherTransfers.length" class="settings-section share-transfers">
          <div class="settings-section-header">
            <div class="section-icon"><ArrowRightLeft :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('share.transfers') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div v-for="t in otherTransfers" :key="t.id" class="share-transfer">
              <div class="share-transfer-icon">
                <Upload v-if="t.direction === 'send'" :size="16" />
                <Download v-else :size="16" />
              </div>
              <div class="share-transfer-body">
                <div class="share-transfer-head">
                  <span class="share-transfer-name">{{ transferName(t) }}</span>
                </div>
                <div class="share-transfer-meta">
                  <span>{{ statusLabel(t) }}</span>
                  <span
                    v-if="showBadge(t)"
                    class="share-badge share-badge--meta"
                    :class="`share-badge--${t.path}`"
                    :title="pathLabel(t.path, false)"
                  >
                    <Zap v-if="t.path === 'direct' || t.path === 'mixed'" :size="12" />
                    <Radio v-else :size="12" />
                    <span class="share-badge-label">{{ pathLabel(t.path) }}</span>
                  </span>
                  <template v-if="t.total > 0">
                    <span class="share-transfer-sep">·</span>
                    <span>{{ transferProgressText(t) }}</span>
                  </template>
                </div>
                <Progress
                  v-if="t.status === 'transferring' && t.total > 0"
                  :model-value="percent(t)"
                  class="share-progress"
                />
                <Progress
                  v-else-if="t.status === 'terminated' && t.total > 0"
                  :model-value="percent(t)"
                  class="share-progress share-progress--terminated"
                />
                <p v-if="t.error && t.status === 'error'" class="share-error">{{ t.error }}</p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                class="share-transfer-action"
                :aria-label="isFinished(t) ? $t('share.dismiss') : $t('share.cancel')"
                @click="onTransferAction(t)"
              >
                <X :size="16" />
              </Button>
            </div>
          </div>
        </section>
        </template>

        <LoginModal v-model:open="showLoginModal" @success="onLoginSuccess" />
      </div>
    </main>
  </div>
</template>

<script lang="ts">
import {
	ArrowRightLeft,
	CheckCircle2,
	Copy,
	Download,
	File,
	FilePlus2,
	Folder,
	FolderDown,
	FolderPlus,
	Loader2,
	LogIn,
	Radio,
	ScanLine,
	Search,
	Share2,
	Upload,
	X,
	XCircle,
	Zap,
} from "@lucide/vue";
import { bytesToSize, getApiErrorMessage } from "@shared/utils";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, Motion } from "motion-v";
import QRCode from "qrcode";
import { defineComponent } from "vue";
import LoginModal from "@/components/Auth/LoginModal.vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import is from "@/shims/platform";
import { useAuthStore } from "@/store/auth";
import {
	parseShareInput,
	type ShareDirection,
	type SharePath,
	type ShareTransfer,
	useShareStore,
} from "@/store/share";
import { safUriToFilesystemPath } from "@/utils/native";

export default defineComponent({
	name: "mo-share",
	components: {
		Tabs,
		TabsList,
		TabsTrigger,
		AnimatePresence,
		Motion,
		Button,
		Input,
		Progress,
		LoginModal,
		ArrowRightLeft,
		Copy,
		Download,
		File,
		FilePlus2,
		Folder,
		FolderDown,
		FolderPlus,
		Loader2,
		LogIn,
		Radio,
		ScanLine,
		Search,
		Share2,
		Upload,
		X,
		Zap,
	},
	data() {
		return {
			store: useShareStore(),
			authStore: useAuthStore(),
			showLoginModal: false,
			selected: [] as { path: string; isDir: boolean }[],
			codeInput: "",
			preparing: false,
			receiving: false,
			scanning: false,
			qrDataUrl: "",
			roleSlide: "forward" as "forward" | "back",
		};
	},
	computed: {
		isLoggedIn(): boolean {
			return this.authStore.isLoggedIn;
		},
		isAndroid(): boolean {
			return is.android();
		},
		activeSend(): ShareTransfer | undefined {
			return this.store.transfers.find(
				(t) =>
					t.direction === "send" &&
					["preparing", "waiting", "connecting", "transferring"].includes(
						t.status,
					),
			);
		},
		activeSendUrl(): string {
			const send = this.activeSend;
			if (!send) {
				return "";
			}
			if (send.deviceCode) {
				return `${this.store.serverUrl.replace(/\/$/, "")}/share/${send.deviceCode}`;
			}
			return send.url || "";
		},
		otherTransfers(): ShareTransfer[] {
			const activeId = this.activeSend?.id;
			return this.store.transfers.filter((t) => t.id !== activeId);
		},
	},
	watch: {
		activeSendUrl: {
			immediate: true,
			handler(url: string) {
				this.makeQr(url);
			},
		},
	},
	mounted() {
		this.store.ensureListener();
	},
	methods: {
		onLoginSuccess() {
			this.showLoginModal = false;
			this.store.resumeAfterLogin();
		},
		async makeQr(url: string) {
			if (!url) {
				this.qrDataUrl = "";
				return;
			}
			try {
				this.qrDataUrl = await QRCode.toDataURL(url, {
					margin: 1,
					width: 220,
					errorCorrectionLevel: "M",
				});
			} catch {
				this.qrDataUrl = "";
			}
		},
		onRoleChange(value: string | number) {
			const next = value === "receive" ? "receive" : "send";
			if (next !== this.store.role) {
				this.roleSlide = next === "receive" ? "forward" : "back";
			}
			this.store.role = next;
		},
		panelAnimate() {
			return { opacity: 1, x: 0 };
		},
		panelTransition() {
			return { duration: 0.24, ease: [0.22, 0.61, 0.36, 1] };
		},
		panelInitial(role: ShareDirection) {
			const offset = 16;
			if (this.roleSlide === "forward") {
				return role === "receive"
					? { opacity: 0, x: offset }
					: { opacity: 0, x: -offset };
			}
			return role === "send"
				? { opacity: 0, x: -offset }
				: { opacity: 0, x: offset };
		},
		panelExit(role: ShareDirection) {
			const offset = 16;
			if (this.roleSlide === "forward") {
				return role === "send"
					? { opacity: 0, x: -offset }
					: { opacity: 0, x: offset };
			}
			return role === "receive"
				? { opacity: 0, x: offset }
				: { opacity: 0, x: -offset };
		},
		async pickDirectory(): Promise<string | null> {
			if (is.android()) {
				const granted = await invoke<boolean>("ensure_android_storage_access");
				if (!granted) {
					this.$msg.warning(this.$t("app.android-storage-access-required"));
					return null;
				}
				const selected = await invoke<string | null>(
					"select_android_directory",
				);
				return selected ? safUriToFilesystemPath(String(selected)) : null;
			}
			const selected = await open({ directory: true, multiple: false });
			return selected ? String(selected) : null;
		},
		addPaths(paths: string[], isDir: boolean) {
			for (const path of paths) {
				if (!this.selected.some((s) => s.path === path)) {
					this.selected.push({ path, isDir });
				}
			}
		},
		async onAddFiles() {
			try {
				if (is.android()) {
					const granted = await invoke<boolean>(
						"ensure_android_storage_access",
					);
					if (!granted) {
						this.$msg.warning(this.$t("app.android-storage-access-required"));
						return;
					}
				}
				let picked = await open({ multiple: true });
				if (!picked) {
					return;
				}
				let paths = (Array.isArray(picked) ? picked : [picked]).map(String);
				if (is.android()) {
					const mapped = paths.map(safUriToFilesystemPath);
					const ready: string[] = [];
					const contentUris = mapped.filter((p) => p.startsWith("content://"));
					for (const path of mapped) {
						if (!path.startsWith("content://")) {
							ready.push(path);
						}
					}
					if (contentUris.length) {
						const staged = await invoke<string[]>("stage_android_share_paths", {
							paths: contentUris,
						});
						ready.push(...staged);
					}
					if (!ready.length) {
						this.$msg.warning(this.$t("share.unsupported-source"));
						return;
					}
					this.addPaths(ready, false);
					return;
				}
				this.addPaths(paths, false);
			} catch (err) {
				this.$msg.error(`${err}`);
			}
		},
		async onAddFolder() {
			try {
				const dir = await this.pickDirectory();
				if (dir) {
					this.addPaths([dir], true);
				}
			} catch (err) {
				this.$msg.error(`${err}`);
			}
		},
		removeSelected(idx: number) {
			this.selected.splice(idx, 1);
		},
		baseName(path: string): string {
			const parts = path.split(/[/\\]/).filter(Boolean);
			return parts.length ? parts[parts.length - 1] : path;
		},
		async onShare() {
			if (!this.selected.length) {
				return;
			}
			try {
				this.preparing = true;
				await this.store.startSend(this.selected.map((s) => s.path));
				this.selected = [];
			} catch (err) {
				this.$msg.error(`${err}`);
			} finally {
				this.preparing = false;
			}
		},
		async onLookup() {
			if (!this.codeInput.trim()) {
				return;
			}
			await this.store.lookupCode(this.codeInput);
		},
		async onScanQr() {
			if (this.scanning || this.store.resolving) {
				return;
			}
			this.scanning = true;
			try {
				const { Format, requestPermissions, scan } = await import(
					"@tauri-apps/plugin-barcode-scanner"
				);
				await requestPermissions();
				const result = await scan({
					formats: [Format.QRCode],
					cameraDirection: "back",
				});
				this.codeInput = parseShareInput(result.content);
				if (!this.codeInput) {
					this.$msg.warning(this.$t("share.scan-failed"));
					return;
				}
				await this.onLookup();
			} catch (err) {
				const message = getApiErrorMessage(err, this.$t("share.scan-failed"));
				if (!/cancel|denied|permission|abort/i.test(message)) {
					this.$msg.error(message);
				}
			} finally {
				this.scanning = false;
			}
		},
		async onReceive() {
			try {
				const dir = await this.pickDirectory();
				if (!dir) {
					return;
				}
				this.receiving = true;
				await this.store.receivePending(dir);
				this.codeInput = "";
			} catch (err) {
				this.$msg.error(`${err}`);
			} finally {
				this.receiving = false;
			}
		},
		async copy(text: string) {
			if (!text) {
				return;
			}
			try {
				const { writeText } = await import(
					"@tauri-apps/plugin-clipboard-manager"
				);
				await writeText(text);
				this.$msg.success(this.$t("share.copied"));
			} catch {
				try {
					await navigator.clipboard.writeText(text);
					this.$msg.success(this.$t("share.copied"));
				} catch (err) {
					this.$msg.error(`${err}`);
				}
			}
		},
		onTransferAction(t: ShareTransfer) {
			if (this.isFinished(t)) {
				this.store.remove(t.id);
			} else {
				this.store.cancel(t.id);
			}
		},
		isFinished(t: ShareTransfer): boolean {
			return ["completed", "terminated", "error", "cancelled"].includes(
				t.status,
			);
		},
		showBadge(t: ShareTransfer): boolean {
			return (
				t.path !== "unknown" && ["transferring", "completed"].includes(t.status)
			);
		},
		pathLabel(path: SharePath, compact = true): string {
			const useShort = compact || this.isAndroid;
			if (path === "direct" || path === "mixed") {
				return useShort
					? this.$t("share.path-direct-short")
					: this.$t("share.path-direct");
			}
			if (path === "relay") {
				return useShort
					? this.$t("share.path-relay-short")
					: this.$t("share.path-relay");
			}
			return "";
		},
		statusLabel(t: ShareTransfer): string {
			return this.$t(`share.status-${t.status}`);
		},
		shareHint(t: ShareTransfer): string {
			if (t.status === "preparing") {
				return this.$t("share.preparing-hint");
			}
			if (t.status === "waiting") {
				return this.$t("share.waiting-hint");
			}
			if (t.status === "transferring" || t.status === "connecting") {
				return this.$t("share.transferring-hint");
			}
			if (t.status === "completed") {
				return this.$t("share.completed-hint");
			}
			if (t.status === "terminated") {
				return this.$t("share.terminated-hint");
			}
			if (t.status === "cancelled") {
				return this.$t("share.cancelled-hint");
			}
			if (t.status === "error") {
				return t.error || this.$t("share.status-error");
			}
			return "";
		},
		statusIcon(t: ShareTransfer) {
			if (t.status === "completed") {
				return CheckCircle2;
			}
			if (
				t.status === "terminated" ||
				t.status === "error" ||
				t.status === "cancelled"
			) {
				return XCircle;
			}
			if (t.status === "preparing" || t.status === "connecting") {
				return Loader2;
			}
			if (t.status === "waiting") {
				return Radio;
			}
			return Zap;
		},
		transferName(t: ShareTransfer): string {
			if (t.files.length === 0) {
				return this.$t("share.untitled");
			}
			if (t.files.length === 1) {
				return t.files[0].name;
			}
			return this.$t("share.n-files", { count: t.files.length });
		},
		percent(t: ShareTransfer): number {
			if (t.total <= 0) {
				return 0;
			}
			return Math.min(100, Math.round((t.transferred / t.total) * 100));
		},
		formatSpeed(bps: number): string {
			if (!bps || bps <= 0) {
				return "";
			}
			return `${bytesToSize(bps)}/s`;
		},
		transferProgressText(t: ShareTransfer): string {
			if (t.total <= 0) {
				return "";
			}
			let text = `${this.formatBytes(t.transferred)} / ${this.formatBytes(t.total)}`;
			if (t.status === "transferring" && t.speedBps > 0) {
				text += ` · ${this.formatSpeed(t.speedBps)}`;
			}
			return text;
		},
		formatBytes(bytes: number): string {
			return bytesToSize(bytes);
		},
	},
});
</script>

<style scoped>
.share-page {
  flex: 1 1 auto;
  min-height: 0;
  overflow: hidden;
}

.share-page > .panel-content {
  flex: 1 1 0;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
}

.share-body {
  max-width: 760px;
  margin: 0 auto;
  padding: 16px 20px 32px;
  width: 100%;
}

.share-tabs-list {
  display: inline-flex;
  gap: 4px;
  padding: 4px;
  border-radius: 10px;
  background: var(--muted, rgba(127, 127, 127, 0.1));
  margin-bottom: 16px;
}

.share-tab-stage {
  position: relative;
  overflow: hidden;
}

.share-tab-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.share-primary-action {
  width: 100%;
}

.share-picker {
  border: 1px dashed var(--border, rgba(127, 127, 127, 0.3));
  border-radius: 12px;
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.share-picker--empty {
  align-items: center;
  justify-content: center;
  text-align: center;
  padding: 28px 14px;
}

.share-picker-icon {
  color: var(--muted-foreground);
  opacity: 0.7;
}

.share-picker-hint {
  color: var(--muted-foreground);
  font-size: 13px;
  margin: 0;
}

.share-picker-actions {
  display: flex;
  gap: 8px;
}

.share-pick-btn {
  flex: 1;
}

.share-staged-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 240px;
  overflow-y: auto;
}

.share-staged-list li {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 8px;
  background: var(--muted, rgba(127, 127, 127, 0.08));
  font-size: 13px;
}

.share-staged-icon {
  flex: 0 0 auto;
  color: var(--muted-foreground);
}

.share-staged-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-staged-remove {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--muted-foreground);
  cursor: pointer;
}

.share-staged-remove:hover {
  background: var(--border, rgba(127, 127, 127, 0.18));
  color: var(--foreground);
}

.share-cta-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 14px;
}

.share-selected-count {
  flex: 0 0 auto;
  font-size: 13px;
  color: var(--muted-foreground);
}

.share-cta-row .share-primary-action {
  flex: 1;
}

.share-result-card {
  border: 1px solid var(--border, rgba(127, 127, 127, 0.2));
  border-radius: 14px;
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.share-qr {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  margin-bottom: 20px;
}
.share-qr img {
  border-radius: 12px;
  background: #fff;
  padding: 10px;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.12);
}
.share-qr-hint {
  margin: 0;
  font-size: 12px;
  color: var(--muted-foreground);
}

.share-result-head {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-bottom: 12px;
  margin-bottom: 4px;
  border-bottom: 1px solid var(--border, rgba(127, 127, 127, 0.14));
}

.share-result-status {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: 0;
}

.share-path-row {
  display: flex;
  padding-left: 30px;
}

.share-result-status-icon {
  flex: 0 0 auto;
  color: var(--primary, #6750a4);
}

.share-result-status-text {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.share-result-sub {
  font-size: 12px;
  color: var(--muted-foreground);
}

.share-result-bytes {
  font-size: 12px;
  color: var(--muted-foreground);
  margin: 2px 0 0;
}

.share-result-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 12px;
}

.share-code-card {
  border: 1px solid var(--border, rgba(127, 127, 127, 0.2));
  border-radius: 14px;
  padding: 20px;
  background: var(--card, transparent);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.share-code-label {
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--muted-foreground);
  margin: 8px 0 2px;
}

.share-code-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.share-code {
  flex: 1;
  font-size: 28px;
  font-weight: 700;
  letter-spacing: 4px;
  font-variant-numeric: tabular-nums;
  user-select: all;
}

.share-url-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.share-url-input {
  flex: 1;
  font-size: 13px;
}

.share-copy-btn {
  flex: 0 0 auto;
}

.share-status-line {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  font-size: 13px;
  color: var(--muted-foreground);
}

.share-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  max-width: 100%;
}

.share-badge-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-badge--meta {
  padding: 1px 7px;
}

.share-badge--direct,
.share-badge--mixed {
  background: color-mix(in srgb, #2e7d32 18%, transparent);
  color: #2e7d32;
}

.share-badge--relay {
  background: color-mix(in srgb, #ed6c02 20%, transparent);
  color: #b45309;
}

:global(html.dark) .share-badge--direct,
:global(html.dark) .share-badge--mixed {
  background: color-mix(in srgb, #66bb6a 24%, transparent);
  color: #a5d6a7;
}

:global(html.dark) .share-badge--relay {
  background: color-mix(in srgb, #ffa726 24%, transparent);
  color: #ffcc80;
}

.share-progress {
  margin-top: 10px;
}

.share-progress--terminated :deep([role="progressbar"]) {
  opacity: 0.65;
}

.share-code-entry {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.share-code-actions {
  display: flex;
  gap: 8px;
}

.share-scan-btn,
.share-lookup-btn {
  flex: 1;
}

.share-code-field {
  flex: 1;
  text-transform: uppercase;
  letter-spacing: 2px;
  font-weight: 600;
}

.share-error {
  color: var(--destructive, #d32f2f);
  font-size: 13px;
  margin-top: 8px;
}

.share-incoming {
  margin-top: 16px;
  border-top: 1px solid var(--border, rgba(127, 127, 127, 0.2));
  padding-top: 16px;
}

.share-incoming-title {
  font-weight: 600;
  margin-bottom: 8px;
}

.share-file-list {
  list-style: none;
  padding: 0;
  margin: 0 0 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.share-file-list li {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}

.share-file-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-file-size {
  flex: 0 0 auto;
  color: var(--muted-foreground);
}

.share-incoming-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.share-transfers {
  margin-top: 32px;
}

.share-login-required {
  margin-top: 8px;
}

.share-login-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: 10px;
  padding: 40px 24px;
  border: 1px solid var(--border, rgba(127, 127, 127, 0.2));
  border-radius: 14px;
}

.share-login-icon {
  display: grid;
  place-items: center;
  width: 56px;
  height: 56px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--primary, #6750a4) 14%, transparent);
  color: var(--primary, #6750a4);
}

.share-login-title {
  margin: 4px 0 0;
  font-size: 16px;
  font-weight: 700;
}

.share-login-desc {
  margin: 0;
  max-width: 360px;
  color: var(--muted-foreground);
  font-size: 13px;
  line-height: 1.5;
}

.share-login-btn {
  margin-top: 8px;
}

.share-transfer {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 12px 0;
  border-bottom: 1px solid var(--border, rgba(127, 127, 127, 0.14));
}

.share-transfer:last-child {
  border-bottom: 0;
}

.share-transfer-icon {
  flex: 0 0 auto;
  margin-top: 2px;
  color: var(--muted-foreground);
}

.share-transfer-body {
  flex: 1;
  min-width: 0;
}

.share-transfer-head {
  min-width: 0;
}

.share-transfer-name {
  display: block;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-transfer-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 6px;
  font-size: 12px;
  color: var(--muted-foreground);
  margin-top: 4px;
}

.share-transfer-sep {
  opacity: 0.55;
}

.share-transfer-action {
  flex: 0 0 auto;
}
</style>
