<template>
  <div class="content panel panel-layout panel-layout--v">
    <mo-enter tag="header" preset="fadeInDown" class="panel-header">
      <h4 class="hidden-xs-only">{{ title }}</h4>
      <mo-subnav-switcher :title="title" :subnavs="subnavs" class="hidden-sm-and-up" />
    </mo-enter>
    <main class="panel-content">
      <div class="form-preference">
        <!-- Sinks Section -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Cloud :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('cloudSinks.sinks') }}</h3>
              <p class="section-desc">{{ $t('cloudSinks.sinksHint') }}</p>
            </div>
            <Button size="sm" @click="openCreate">
              <Plus :size="14" class="mr-1" />
              {{ $t('cloudSinks.addSink') }}
            </Button>
          </div>
          <div class="settings-section-content">
            <div v-if="store.sinks.length === 0" class="empty-state">
              <CloudOff :size="28" class="empty-state-icon" />
              <p class="empty-state-title">{{ $t('cloudSinks.empty') }}</p>
              <p class="empty-state-hint">{{ $t('cloudSinks.emptyHint') }}</p>
            </div>
            <div v-else class="sink-list">
              <div
                v-for="sink in store.sinks"
                :key="sink.id"
                class="sink-card"
                :class="{ 'sink-card--default': store.defaultSinkId === sink.id }"
              >
                <div class="sink-card-icon">
                  <component :is="protocolIcon(sink.config.kind)" :size="18" />
                </div>
                <div class="sink-card-main">
                  <div class="sink-card-line">
                    <span class="sink-card-label">{{ sink.label || sink.id }}</span>
                    <span class="sink-card-kind">{{ sink.config.kind.toUpperCase() }}</span>
                    <span v-if="store.defaultSinkId === sink.id" class="sink-card-badge">
                      <Star :size="10" fill="currentColor" />
                      {{ $t('cloudSinks.defaultBadge') }}
                    </span>
                  </div>
                  <div class="sink-card-meta" :title="sinkMeta(sink)">
                    {{ sinkMeta(sink) }}
                  </div>
                </div>
                <div class="sink-card-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    :disabled="testingId === sink.id"
                    @click="onTest(sink.id)"
                  >
                    <Loader2 v-if="testingId === sink.id" :size="13" class="animate-spin" />
                    <Plug v-else :size="13" />
                    <span class="ml-1">{{ $t('cloudSinks.test') }}</span>
                  </Button>
                  <Button
                    v-if="store.defaultSinkId !== sink.id"
                    size="sm"
                    variant="ghost"
                    @click="onSetDefault(sink.id)"
                  >
                    <Star :size="13" />
                    <span class="ml-1">{{ $t('cloudSinks.makeDefault') }}</span>
                  </Button>
                  <Button size="sm" variant="ghost" @click="openEdit(sink)">
                    <Pencil :size="13" />
                    <span class="ml-1">{{ $t('cloudSinks.edit') }}</span>
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    class="action-danger"
                    @click="askRemove(sink)"
                  >
                    <Trash2 :size="13" />
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Recent Uploads -->
        <div class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Activity :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('cloudSinks.recentJobs') }}</h3>
              <p class="section-desc">{{ $t('cloudSinks.recentJobsHint') }}</p>
            </div>
            <Button
              size="sm"
              variant="outline"
              :disabled="store.jobs.length === 0"
              @click="store.clearHistory()"
            >
              {{ $t('cloudSinks.clearHistory') }}
            </Button>
          </div>
          <div class="settings-section-content">
            <div v-if="store.jobs.length === 0" class="empty-state empty-state--small">
              <Inbox :size="22" class="empty-state-icon" />
              <p class="empty-state-hint">{{ $t('cloudSinks.noJobs') }}</p>
            </div>
            <div v-else class="job-list">
              <div
                v-for="job in store.jobs.slice(0, 20)"
                :key="job.id"
                class="job-row"
              >
                <span class="job-pill" :data-status="job.status">{{
                  jobStatusLabel(job.status)
                }}</span>
                <div class="job-meta">
                  <div class="job-name" :title="jobName(job)">{{ jobName(job) }}</div>
                  <Progress :model-value="jobPercent(job)" class="job-progress" />
                </div>
                <div class="job-stats">
                  <span class="job-percent">{{ jobPercent(job) }}%</span>
                  <span class="job-size">{{ jobSize(job) }}</span>
                </div>
                <div class="job-actions">
                  <Button
                    v-if="job.status === 'active' || job.status === 'queued'"
                    size="sm"
                    variant="ghost"
                    @click="store.cancelJob(job.id)"
                  >
                    <X :size="13" />
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </main>

    <!-- Add / Edit Dialog -->
    <Dialog :open="dialogOpen" @update:open="onDialogOpenChange">
      <DialogContent class="cloud-sink-dialog" :show-close-button="true">
        <DialogHeader>
          <DialogTitle>{{ dialogTitle }}</DialogTitle>
          <p class="dialog-subtitle">{{ $t('cloudSinks.sinksHint') }}</p>
        </DialogHeader>

        <div class="dialog-form">
          <!-- Identity row -->
          <div class="grid-2">
            <div class="dialog-field">
              <Label for="cs-label">{{ $t('cloudSinks.label') }}</Label>
              <Input
                id="cs-label"
                v-model="form.label"
                :placeholder="$t('cloudSinks.labelPlaceholder')"
                autocomplete="off"
              />
            </div>
            <div class="dialog-field">
              <Label for="cs-protocol">{{ $t('cloudSinks.protocol') }}</Label>
              <Select v-model="form.kind">
                <SelectTrigger id="cs-protocol">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="webdav">WebDAV</SelectItem>
                  <SelectItem value="s3">S3</SelectItem>
                  <SelectItem value="sftp">SFTP</SelectItem>
                  <SelectItem value="ftp">FTP / FTPS</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <!-- WebDAV -->
          <template v-if="form.kind === 'webdav'">
            <div class="dialog-field">
              <Label for="cs-endpoint">{{ $t('cloudSinks.endpoint') }}</Label>
              <Input
                id="cs-endpoint"
                v-model="form.webdav.endpoint"
                placeholder="https://dav.example.com"
                autocomplete="off"
              />
            </div>
            <div class="dialog-field">
              <Label for="cs-base">{{ $t('cloudSinks.basePath') }}</Label>
              <Input
                id="cs-base"
                v-model="form.webdav.basePath"
                placeholder="/risuko"
                autocomplete="off"
              />
              <p class="dialog-hint">{{ $t('cloudSinks.basePathHint') }}</p>
            </div>
            <div class="grid-2">
              <div class="dialog-field">
                <Label for="cs-user">{{ $t('cloudSinks.username') }}</Label>
                <Input id="cs-user" v-model="form.webdav.username" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label for="cs-pass">{{ $t('cloudSinks.password') }}</Label>
                <Input
                  id="cs-pass"
                  v-model="form.webdav.password"
                  type="password"
                  autocomplete="new-password"
                />
              </div>
            </div>
            <div class="dialog-field-toggle">
              <ui-checkbox v-model="form.webdav.insecure" />
              <div>
                <span class="toggle-title">{{ $t('cloudSinks.allowInsecure') }}</span>
                <span class="toggle-hint">{{ $t('cloudSinks.allowInsecureHint') }}</span>
              </div>
            </div>
          </template>

          <!-- S3 -->
          <template v-else-if="form.kind === 's3'">
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.s3Endpoint') }}</Label>
              <Input
                v-model="form.s3.endpoint"
                :placeholder="$t('cloudSinks.s3EndpointPlaceholder')"
                autocomplete="off"
              />
            </div>
            <div class="grid-2">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.s3Region') }}</Label>
                <Input
                  v-model="form.s3.region"
                  :placeholder="$t('cloudSinks.s3RegionPlaceholder')"
                  autocomplete="off"
                />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.s3Bucket') }}</Label>
                <Input v-model="form.s3.bucket" autocomplete="off" />
              </div>
            </div>
            <div class="grid-2">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.s3AccessKey') }}</Label>
                <Input v-model="form.s3.accessKeyId" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.s3SecretKey') }}</Label>
                <Input
                  v-model="form.s3.secretAccessKey"
                  type="password"
                  autocomplete="new-password"
                />
              </div>
            </div>
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.s3Prefix') }}</Label>
              <Input v-model="form.s3.prefix" placeholder="uploads/" autocomplete="off" />
              <p class="dialog-hint">{{ $t('cloudSinks.s3PrefixHint') }}</p>
            </div>
            <div class="dialog-field-toggle">
              <ui-checkbox v-model="form.s3.forcePathStyle" />
              <div>
                <span class="toggle-title">{{ $t('cloudSinks.s3PathStyle') }}</span>
                <span class="toggle-hint">{{ $t('cloudSinks.s3PathStyleHint') }}</span>
              </div>
            </div>
          </template>

          <!-- SFTP -->
          <template v-else-if="form.kind === 'sftp'">
            <div class="grid-host-port">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.host') }}</Label>
                <Input v-model="form.sftp.host" placeholder="sftp.example.com" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.port') }}</Label>
                <Input
                  v-model.number="form.sftp.port"
                  type="number"
                  min="1"
                  max="65535"
                  placeholder="22"
                />
              </div>
            </div>
            <div class="grid-2">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.username') }}</Label>
                <Input v-model="form.sftp.username" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.password') }}</Label>
                <Input
                  v-model="form.sftp.password"
                  type="password"
                  autocomplete="new-password"
                />
              </div>
            </div>
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.sftpBasePath') }}</Label>
              <Input
                v-model="form.sftp.basePath"
                :placeholder="$t('cloudSinks.sftpBasePathPlaceholder')"
                autocomplete="off"
              />
            </div>
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.privateKey') }}</Label>
              <textarea
                v-model="form.sftp.privateKey"
                class="dialog-textarea"
                rows="3"
                placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                autocomplete="off"
                spellcheck="false"
              />
              <p class="dialog-hint">{{ $t('cloudSinks.privateKeyHint') }}</p>
            </div>
          </template>

          <!-- FTP -->
          <template v-else-if="form.kind === 'ftp'">
            <div class="grid-host-port">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.host') }}</Label>
                <Input v-model="form.ftp.host" placeholder="ftp.example.com" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.port') }}</Label>
                <Input
                  v-model.number="form.ftp.port"
                  type="number"
                  min="1"
                  max="65535"
                  placeholder="21"
                />
              </div>
            </div>
            <div class="grid-2">
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.username') }}</Label>
                <Input v-model="form.ftp.username" autocomplete="off" />
              </div>
              <div class="dialog-field">
                <Label>{{ $t('cloudSinks.password') }}</Label>
                <Input
                  v-model="form.ftp.password"
                  type="password"
                  autocomplete="new-password"
                />
              </div>
            </div>
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.basePath') }}</Label>
              <Input v-model="form.ftp.basePath" placeholder="/uploads" autocomplete="off" />
            </div>
            <div class="dialog-field-toggle">
              <ui-checkbox v-model="form.ftp.secure" />
              <div>
                <span class="toggle-title">{{ $t('cloudSinks.ftpSecure') }}</span>
                <span class="toggle-hint">{{ $t('cloudSinks.ftpSecureHint') }}</span>
              </div>
            </div>
          </template>

          <hr class="dialog-sep" />

          <!-- Post-action -->
          <div class="grid-2">
            <div class="dialog-field">
              <Label>{{ $t('cloudSinks.postAction') }}</Label>
              <Select v-model="form.postAction">
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="keep">{{ $t('cloudSinks.postKeep') }}</SelectItem>
                  <SelectItem value="trash">{{ $t('cloudSinks.postTrash') }}</SelectItem>
                  <SelectItem value="move">{{ $t('cloudSinks.postMove') }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div v-if="form.postAction === 'move'" class="dialog-field">
              <Label>{{ $t('cloudSinks.moveTarget') }}</Label>
              <Input
                v-model="form.moveTarget"
                :placeholder="$t('cloudSinks.moveTargetPlaceholder')"
              />
            </div>
          </div>

          <div v-if="dialogError" class="dialog-error">
            <AlertCircle :size="14" class="shrink-0" />
            <span>{{ dialogError }}</span>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" :disabled="saving" @click="dialogOpen = false">
            {{ $t('cloudSinks.cancel') }}
          </Button>
          <Button :disabled="saving" @click="onSave">
            <Loader2 v-if="saving" :size="13" class="animate-spin mr-1" />
            {{ saving ? $t('cloudSinks.saving') : $t('cloudSinks.save') }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- Confirm remove -->
    <Dialog
      :open="!!removingSink"
      @update:open="(v: boolean) => { if (!v) removingSink = null }"
    >
      <DialogContent class="cloud-sink-confirm" :show-close-button="false">
        <DialogHeader>
          <DialogTitle>{{ $t('cloudSinks.confirmRemoveTitle') }}</DialogTitle>
          <p class="dialog-subtitle">{{ $t('cloudSinks.confirmRemove') }}</p>
        </DialogHeader>
        <div v-if="removingSink" class="confirm-target">
          <strong>{{ removingSink.label || removingSink.id }}</strong>
          <span class="confirm-target-meta">{{ sinkMeta(removingSink) }}</span>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="removingSink = null">
            {{ $t('cloudSinks.cancel') }}
          </Button>
          <Button variant="destructive" @click="onRemoveConfirmed">
            {{ $t('cloudSinks.delete') }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>

<script lang="ts">
import type {
	SinkConfig,
	UploadJob,
	UploadJobStatus,
	UploadProtocol,
	UploadSinkRecord,
} from "@shared/types/upload";
import { bytesToSize } from "@shared/utils";
import {
	Activity,
	AlertCircle,
	Cloud,
	CloudOff,
	Database,
	HardDrive,
	Inbox,
	Loader2,
	Pencil,
	Plug,
	Plus,
	Server,
	Star,
	Terminal,
	Trash2,
	X,
} from "lucide-vue-next";
import { defineComponent } from "vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { useUploadSinkStore } from "@/store/uploadSink";

interface FormState {
	id: string;
	label: string;
	kind: UploadProtocol;
	webdav: {
		endpoint: string;
		basePath: string;
		username: string;
		password: string;
		insecure: boolean;
	};
	s3: {
		endpoint: string;
		region: string;
		bucket: string;
		accessKeyId: string;
		secretAccessKey: string;
		prefix: string;
		forcePathStyle: boolean;
	};
	sftp: {
		host: string;
		port: number;
		username: string;
		password: string;
		privateKey: string;
		basePath: string;
	};
	ftp: {
		host: string;
		port: number;
		username: string;
		password: string;
		basePath: string;
		secure: boolean;
	};
	postAction: "keep" | "trash" | "move";
	moveTarget: string;
}

function emptyForm(): FormState {
	return {
		id: "",
		label: "",
		kind: "webdav",
		webdav: {
			endpoint: "",
			basePath: "",
			username: "",
			password: "",
			insecure: false,
		},
		s3: {
			endpoint: "",
			region: "us-east-1",
			bucket: "",
			accessKeyId: "",
			secretAccessKey: "",
			prefix: "",
			forcePathStyle: false,
		},
		sftp: {
			host: "",
			port: 22,
			username: "",
			password: "",
			privateKey: "",
			basePath: "",
		},
		ftp: {
			host: "",
			port: 21,
			username: "",
			password: "",
			basePath: "",
			secure: false,
		},
		postAction: "keep",
		moveTarget: "",
	};
}

export default defineComponent({
	name: "mo-cloud-sinks",
	components: {
		Activity,
		AlertCircle,
		Button,
		Cloud,
		CloudOff,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		Inbox,
		Input,
		Label,
		Loader2,
		Pencil,
		Plug,
		Plus,
		Progress,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Star,
		Trash2,
		X,
	},
	data() {
		return {
			dialogOpen: false,
			editingId: null as string | null,
			form: emptyForm(),
			saving: false,
			dialogError: "" as string,
			testingId: null as string | null,
			removingSink: null as UploadSinkRecord | null,
		};
	},
	computed: {
		store() {
			return useUploadSinkStore();
		},
		title(): string {
			return this.$t("preferences.cloudSinks") as string;
		},
		subnavs(): { name: string; path: string; current: string }[] {
			return [];
		},
		dialogTitle(): string {
			return this.editingId
				? (this.$t("cloudSinks.editSink") as string)
				: (this.$t("cloudSinks.addSink") as string);
		},
	},
	async mounted() {
		await this.store.fetchAll();
		await this.store.initEventListeners();
	},
	beforeUnmount() {
		this.store.cleanupEventListeners();
	},
	methods: {
		protocolIcon(kind: string) {
			switch (kind) {
				case "webdav":
					return Server;
				case "s3":
					return Database;
				case "sftp":
					return Terminal;
				case "ftp":
					return HardDrive;
				default:
					return Cloud;
			}
		},
		sinkMeta(sink: UploadSinkRecord): string {
			const c = sink.config;
			const joinPath = (head: string, tail?: string | null) => {
				const t = (tail ?? "").trim();
				if (!t) {
					return head;
				}
				return `${head.replace(/\/+$/, "")}/${t.replace(/^\/+/, "")}`;
			};
			switch (c.kind) {
				case "webdav":
					return joinPath(c.endpoint, c.basePath);
				case "s3":
					return joinPath(joinPath(c.endpoint, c.bucket), c.prefix);
				case "sftp":
					return joinPath(
						`sftp://${c.username}@${c.host}:${c.port}`,
						c.basePath,
					);
				case "ftp":
					return joinPath(
						`${c.secure ? "ftps" : "ftp"}://${c.username || "anonymous"}@${c.host}:${c.port}`,
						c.basePath,
					);
				default:
					return "";
			}
		},
		openCreate() {
			this.form = emptyForm();
			this.editingId = null;
			this.dialogError = "";
			this.dialogOpen = true;
		},
		openEdit(sink: UploadSinkRecord) {
			const f = emptyForm();
			f.id = sink.id;
			f.label = sink.label;
			f.postAction = sink.postAction;
			f.moveTarget = sink.moveTarget ?? "";
			f.kind = sink.config.kind;
			const c = sink.config;
			if (c.kind === "webdav") {
				f.webdav.endpoint = c.endpoint;
				f.webdav.basePath = c.basePath ?? "";
				f.webdav.username = c.username ?? "";
				f.webdav.password = c.password ?? "";
				f.webdav.insecure = !!c.insecure;
			} else if (c.kind === "s3") {
				f.s3.endpoint = c.endpoint;
				f.s3.region = c.region;
				f.s3.bucket = c.bucket;
				f.s3.accessKeyId = c.accessKeyId;
				f.s3.secretAccessKey = c.secretAccessKey;
				f.s3.prefix = c.prefix ?? "";
				f.s3.forcePathStyle = !!c.forcePathStyle;
			} else if (c.kind === "sftp") {
				f.sftp.host = c.host;
				f.sftp.port = c.port || 22;
				f.sftp.username = c.username;
				f.sftp.password = c.password ?? "";
				f.sftp.privateKey = c.privateKey ?? "";
				f.sftp.basePath = c.basePath ?? "";
			} else if (c.kind === "ftp") {
				f.ftp.host = c.host;
				f.ftp.port = c.port || 21;
				f.ftp.username = c.username ?? "";
				f.ftp.password = c.password ?? "";
				f.ftp.basePath = c.basePath ?? "";
				f.ftp.secure = !!c.secure;
			}
			this.form = f;
			this.editingId = sink.id;
			this.dialogError = "";
			this.dialogOpen = true;
		},
		onDialogOpenChange(open: boolean) {
			if (!open) {
				this.dialogOpen = false;
			}
		},
		buildConfig(): SinkConfig {
			switch (this.form.kind) {
				case "webdav":
					return {
						kind: "webdav",
						endpoint: this.form.webdav.endpoint.trim(),
						basePath: this.form.webdav.basePath.trim(),
						username: this.form.webdav.username || null,
						password: this.form.webdav.password || null,
						insecure: this.form.webdav.insecure,
					};
				case "s3":
					return {
						kind: "s3",
						endpoint: this.form.s3.endpoint.trim(),
						region: this.form.s3.region.trim(),
						bucket: this.form.s3.bucket.trim(),
						accessKeyId: this.form.s3.accessKeyId.trim(),
						secretAccessKey: this.form.s3.secretAccessKey,
						prefix: this.form.s3.prefix.trim(),
						forcePathStyle: this.form.s3.forcePathStyle,
					};
				case "sftp":
					return {
						kind: "sftp",
						host: this.form.sftp.host.trim(),
						port: Number(this.form.sftp.port) || 22,
						username: this.form.sftp.username.trim(),
						password: this.form.sftp.password || null,
						privateKey: this.form.sftp.privateKey || null,
						basePath: this.form.sftp.basePath.trim(),
					};
				case "ftp":
					return {
						kind: "ftp",
						host: this.form.ftp.host.trim(),
						port: Number(this.form.ftp.port) || 21,
						username: this.form.ftp.username || null,
						password: this.form.ftp.password || null,
						basePath: this.form.ftp.basePath.trim(),
						secure: this.form.ftp.secure,
					};
			}
		},
		validate(): string | null {
			if (!this.form.label.trim()) {
				return this.$t("cloudSinks.errLabelRequired") as string;
			}
			switch (this.form.kind) {
				case "webdav":
					if (!this.form.webdav.endpoint.trim()) {
						return this.$t("cloudSinks.errEndpointRequired") as string;
					}
					return null;
				case "s3":
					if (!this.form.s3.endpoint.trim()) {
						return "S3 endpoint is required.";
					}
					if (!this.form.s3.bucket.trim()) {
						return "S3 bucket is required.";
					}
					if (!this.form.s3.accessKeyId.trim()) {
						return "S3 access key is required.";
					}
					if (!this.form.s3.secretAccessKey.trim()) {
						return "S3 secret key is required.";
					}
					return null;
				case "sftp":
					if (!this.form.sftp.host.trim()) {
						return "SFTP host is required.";
					}
					if (!this.form.sftp.username.trim()) {
						return "SFTP username is required.";
					}
					if (!this.form.sftp.password && !this.form.sftp.privateKey) {
						return "SFTP password or private key is required.";
					}
					return null;
				case "ftp":
					if (!this.form.ftp.host.trim()) {
						return "FTP host is required.";
					}
					return null;
			}
			return null;
		},
		async onSave() {
			this.dialogError = "";
			const err = this.validate();
			if (err) {
				this.dialogError = err;
				return;
			}
			this.saving = true;
			try {
				const config = this.buildConfig();
				if (this.editingId) {
					const existing = this.store.sinks.find(
						(s) => s.id === this.editingId,
					);
					if (!existing) {
						throw new Error("Sink no longer exists");
					}
					const updated: UploadSinkRecord = {
						...existing,
						label: this.form.label.trim(),
						config,
						postAction: this.form.postAction,
						moveTarget: this.form.moveTarget || null,
					};
					await this.store.updateSink(updated);
				} else {
					await this.store.addSink({
						label: this.form.label.trim(),
						config,
						postAction: this.form.postAction,
						moveTarget: this.form.moveTarget || null,
					});
				}
				this.dialogOpen = false;
			} catch (e) {
				this.dialogError = String((e as Error)?.message || e);
			} finally {
				this.saving = false;
			}
		},
		askRemove(sink: UploadSinkRecord) {
			this.removingSink = sink;
		},
		async onRemoveConfirmed() {
			if (!this.removingSink) {
				return;
			}
			const id = this.removingSink.id;
			const label = this.removingSink.label || id;
			this.removingSink = null;
			try {
				await this.store.removeSink(id);
				toast.success(`Removed “${label}”`);
			} catch (e) {
				toast.error(String((e as Error)?.message || e));
			}
		},
		async onTest(id: string) {
			this.testingId = id;
			try {
				await this.store.testSink(id);
				toast.success(this.$t("cloudSinks.testOk") as string);
			} catch (e) {
				toast.error(
					`${this.$t("cloudSinks.testFail")}: ${(e as Error)?.message || e}`,
				);
			} finally {
				this.testingId = null;
			}
		},
		async onSetDefault(id: string) {
			await this.store.setDefaultSink(id);
		},
		jobStatusLabel(status: UploadJobStatus): string {
			const map: Record<UploadJobStatus, string> = {
				queued: "cloudSinks.jobStatusQueued",
				active: "cloudSinks.jobStatusActive",
				complete: "cloudSinks.jobStatusComplete",
				failed: "cloudSinks.jobStatusFailed",
				cancelled: "cloudSinks.jobStatusCancelled",
			};
			return this.$t(map[status]) as string;
		},
		jobName(job: UploadJob): string {
			return job.remoteRelative || job.localPath;
		},
		jobPercent(job: UploadJob): number {
			if (job.status === "complete") {
				return 100;
			}
			if (!job.size) {
				return 0;
			}
			return Math.min(100, Math.round((job.uploaded / job.size) * 100));
		},
		jobSize(job: UploadJob): string {
			if (!job.size) {
				return "—";
			}
			if (
				job.status === "complete" ||
				job.status === "failed" ||
				job.status === "cancelled"
			) {
				return bytesToSize(job.size) as string;
			}
			return `${bytesToSize(job.uploaded)} / ${bytesToSize(job.size)}`;
		},
	},
});
</script>

<style scoped>
.section-desc {
	margin: 2px 0 0;
	font-size: 12px;
	color: hsl(var(--muted-foreground));
}

/* ---------- Sink cards ---------- */
.sink-list {
	display: flex;
	flex-direction: column;
	gap: 8px;
}

.sink-card {
	display: flex;
	align-items: center;
	gap: 12px;
	padding: 12px 14px;
	border: 1px solid hsl(var(--border));
	border-radius: 10px;
	background: hsl(var(--card));
	transition:
		border-color 0.15s ease,
		background-color 0.15s ease,
		box-shadow 0.15s ease;
}

.sink-card:hover {
	border-color: hsl(var(--ring) / 0.5);
	background: hsl(var(--accent) / 0.4);
}

.sink-card--default {
	border-color: hsl(var(--primary) / 0.4);
	box-shadow: 0 0 0 1px hsl(var(--primary) / 0.15);
}

.sink-card-icon {
	width: 36px;
	height: 36px;
	flex: 0 0 36px;
	display: flex;
	align-items: center;
	justify-content: center;
	border-radius: 8px;
	background: hsl(var(--muted));
	color: hsl(var(--muted-foreground));
}

.sink-card-main {
	flex: 1;
	min-width: 0;
}

.sink-card-line {
	display: flex;
	align-items: center;
	gap: 8px;
	flex-wrap: wrap;
}

.sink-card-label {
	font-size: 14px;
	font-weight: 600;
}

.sink-card-kind {
	font-size: 10px;
	letter-spacing: 0.06em;
	font-weight: 600;
	padding: 2px 6px;
	border-radius: 4px;
	background: hsl(var(--muted));
	color: hsl(var(--muted-foreground));
}

.sink-card-badge {
	display: inline-flex;
	align-items: center;
	gap: 3px;
	font-size: 10px;
	font-weight: 600;
	letter-spacing: 0.04em;
	padding: 2px 7px;
	border-radius: 4px;
	background: hsl(var(--primary));
	color: hsl(var(--primary-foreground));
}

.sink-card-meta {
	margin-top: 3px;
	font-size: 12px;
	color: hsl(var(--muted-foreground));
	overflow: hidden;
	text-overflow: ellipsis;
	white-space: nowrap;
	font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.sink-card-actions {
	display: flex;
	align-items: center;
	gap: 2px;
	flex-shrink: 0;
}

.action-danger {
	color: hsl(var(--destructive));
}

.action-danger:hover {
	background: hsl(var(--destructive) / 0.1);
	color: hsl(var(--destructive));
}

/* ---------- Empty states ---------- */
.empty-state {
	display: flex;
	flex-direction: column;
	align-items: center;
	justify-content: center;
	gap: 6px;
	padding: 36px 16px;
	border: 1px dashed hsl(var(--border));
	border-radius: 10px;
	color: hsl(var(--muted-foreground));
}

.empty-state--small {
	padding: 20px 16px;
	gap: 4px;
}

.empty-state-icon {
	color: hsl(var(--muted-foreground) / 0.7);
	margin-bottom: 4px;
}

.empty-state-title {
	font-size: 13px;
	font-weight: 500;
	color: hsl(var(--foreground));
	margin: 0;
}

.empty-state-hint {
	font-size: 12px;
	margin: 0;
}

/* ---------- Job rows ---------- */
.job-list {
	display: flex;
	flex-direction: column;
	gap: 6px;
}

.job-row {
	display: grid;
	grid-template-columns: 80px 1fr auto auto;
	align-items: center;
	gap: 12px;
	padding: 8px 12px;
	border: 1px solid hsl(var(--border));
	border-radius: 8px;
	background: hsl(var(--card));
}

.job-pill {
	font-size: 10px;
	font-weight: 600;
	letter-spacing: 0.04em;
	text-align: center;
	padding: 3px 6px;
	border-radius: 4px;
	background: hsl(var(--muted));
	color: hsl(var(--muted-foreground));
}

.job-pill[data-status="active"] {
	background: hsl(217 91% 60% / 0.15);
	color: hsl(217 91% 60%);
}

.job-pill[data-status="complete"] {
	background: hsl(142 71% 45% / 0.15);
	color: hsl(142 71% 45%);
}

.job-pill[data-status="failed"] {
	background: hsl(var(--destructive) / 0.15);
	color: hsl(var(--destructive));
}

.job-pill[data-status="cancelled"] {
	background: hsl(var(--muted));
	color: hsl(var(--muted-foreground));
}

.job-meta {
	min-width: 0;
}

.job-name {
	font-size: 13px;
	overflow: hidden;
	text-overflow: ellipsis;
	white-space: nowrap;
	margin-bottom: 4px;
}

.job-progress {
	height: 4px;
}

.job-stats {
	display: flex;
	flex-direction: column;
	align-items: flex-end;
	font-variant-numeric: tabular-nums;
	font-size: 11px;
	color: hsl(var(--muted-foreground));
	min-width: 110px;
}

.job-percent {
	font-size: 12px;
	font-weight: 600;
	color: hsl(var(--foreground));
}

.job-actions {
	display: flex;
}

/* ---------- Dialog ---------- */
.cloud-sink-dialog {
	max-width: 720px;
	width: calc(100vw - 48px);
}

.dialog-subtitle {
	font-size: 12px;
	color: hsl(var(--muted-foreground));
	margin: 4px 0 0;
}

.dialog-form {
	display: flex;
	flex-direction: column;
	gap: 12px;
	padding: 4px 0;
	max-height: 65vh;
	overflow-y: auto;
	padding-right: 4px;
}

.dialog-field {
	display: flex;
	flex-direction: column;
	gap: 6px;
	min-width: 0;
}

.grid-2 {
	display: grid;
	grid-template-columns: 1fr 1fr;
	gap: 12px;
}

.grid-host-port {
	display: grid;
	grid-template-columns: 1fr 100px;
	gap: 12px;
}

.dialog-hint {
	font-size: 11px;
	color: hsl(var(--muted-foreground));
	margin: 0;
}

.dialog-sep {
	border: 0;
	border-top: 1px solid hsl(var(--border));
	margin: 4px 0;
}

.dialog-textarea {
	font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
	font-size: 12px;
	padding: 8px 10px;
	border: 1px solid hsl(var(--border));
	border-radius: 6px;
	background: hsl(var(--background));
	color: hsl(var(--foreground));
	resize: vertical;
	min-height: 60px;
	outline: none;
	transition: border-color 0.15s ease;
}

.dialog-textarea:focus {
	border-color: hsl(var(--ring));
	box-shadow: 0 0 0 2px hsl(var(--ring) / 0.2);
}

.dialog-field-toggle {
	display: flex;
	align-items: center;
	gap: 12px;
	padding: 10px 12px;
	border: 1px solid hsl(var(--border));
	border-radius: 8px;
	background: hsl(var(--muted) / 0.3);
}

.dialog-field-toggle > div {
	display: flex;
	flex-direction: column;
	min-width: 0;
}

.toggle-title {
	font-size: 13px;
	font-weight: 500;
	line-height: 1.2;
}

.toggle-hint {
	font-size: 11px;
	color: hsl(var(--muted-foreground));
	margin-top: 2px;
}

.dialog-error {
	display: flex;
	align-items: center;
	gap: 6px;
	font-size: 12px;
	color: hsl(var(--destructive));
	background: hsl(var(--destructive) / 0.1);
	border: 1px solid hsl(var(--destructive) / 0.3);
	border-radius: 6px;
	padding: 8px 10px;
}

/* ---------- Confirm ---------- */
.cloud-sink-confirm {
	max-width: 420px;
}

.confirm-target {
	display: flex;
	flex-direction: column;
	gap: 2px;
	padding: 10px 12px;
	background: hsl(var(--muted) / 0.5);
	border-radius: 6px;
	font-size: 13px;
}

.confirm-target-meta {
	font-size: 11px;
	color: hsl(var(--muted-foreground));
	font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

@media (max-width: 600px) {
	.grid-2,
	.grid-host-port {
		grid-template-columns: 1fr;
	}
}
</style>
