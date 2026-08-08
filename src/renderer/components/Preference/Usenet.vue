<template>
  <div class="content panel panel-layout panel-layout--v">
    <main class="panel-content">
      <div class="form-preference usenet-preferences">
        <section class="settings-section">
          <div class="settings-section-header usenet-section-header">
            <div class="section-icon"><Radio :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.usenet-profiles') }}</h3>
              <p>{{ $t('preferences.usenet-profiles-tips') }}</p>
            </div>
            <Button class="section-add-button" size="sm" @click="openCreateProfile">
              <Plus :size="14" />
              <span>{{ $t('preferences.usenet-add-profile') }}</span>
            </Button>
          </div>

          <div class="settings-section-content">
            <div v-if="profiles.length === 0" class="provider-empty-state">
              <ServerOff :size="28" class="provider-empty-icon" />
              <p class="provider-empty-title">{{ $t('preferences.usenet-no-profiles') }}</p>
              <p class="provider-empty-hint">{{ $t('preferences.usenet-no-profiles-tips') }}</p>
              <Button size="sm" variant="outline" @click="openCreateProfile">
                <Plus :size="14" />
                {{ $t('preferences.usenet-add-profile') }}
              </Button>
            </div>

            <div v-else class="provider-list">
              <article
                v-for="profile in profiles"
                :key="profile.id"
                class="provider-card"
                :class="{ 'provider-card--disabled': !profile.enabled }"
              >
                <div class="provider-card-icon">
                  <Server :size="18" />
                </div>

                <div class="provider-card-main">
                  <div class="provider-card-heading">
                    <span class="provider-card-name">{{ profile.name }}</span>
                    <span class="provider-badge">{{ securityLabel(profile.securityMode) }}</span>
                    <span v-if="!profile.enabled" class="provider-badge provider-badge--muted">
                      {{ $t('preferences.usenet-disabled') }}
                    </span>
                  </div>
                  <div class="provider-endpoint" :title="`${profile.host}:${profile.port}`">
                    {{ profile.host }}:{{ profile.port }}
                  </div>
                  <div class="provider-card-details">
                    <span>{{ $t('preferences.usenet-priority-value', { value: profile.priority }) }}</span>
                    <span>{{ $t('preferences.usenet-connections-value', { value: profile.maxConnections }) }}</span>
                    <span class="provider-credential-state">
                      <KeyRound :size="11" />
                      {{ credentialLabel(profile.id) }}
                    </span>
                  </div>
                </div>

                <div class="provider-card-actions">
                  <div class="provider-enable-control">
                    <span>{{ $t('preferences.usenet-enabled') }}</span>
                    <ui-switch
                      :model-value="profile.enabled"
                      :aria-label="$t('preferences.usenet-enabled')"
                      @change="(value: boolean) => setProfileEnabled(profile, value)"
                    />
                  </div>
                  <Button
                    class="provider-test-button"
                    size="sm"
                    variant="ghost"
                    :disabled="testingId === profile.id"
                    @click="testProfile(profile)"
                  >
                    <Loader2 v-if="testingId === profile.id" :size="14" class="animate-spin" />
                    <Plug v-else :size="14" />
                    <span>{{ $t('preferences.usenet-test') }}</span>
                  </Button>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    :aria-label="$t('preferences.usenet-edit')"
                    :title="$t('preferences.usenet-edit')"
                    @click="openEditProfile(profile)"
                  >
                    <Pencil :size="14" />
                  </Button>
                  <Button
                    class="provider-delete-button"
                    size="icon-sm"
                    variant="ghost"
                    :aria-label="$t('preferences.usenet-remove')"
                    :title="$t('preferences.usenet-remove')"
                    @click="removeProfile(profile)"
                  >
                    <Trash2 :size="14" />
                  </Button>
                </div>
              </article>
            </div>
          </div>
        </section>

        <section class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><Archive :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.usenet-cleanup') }}</h3>
              <p>{{ $t('preferences.usenet-cleanup-tips') }}</p>
            </div>
          </div>

          <div class="settings-section-content">
            <div class="settings-select-group settings-select-group--stack cleanup-select-group">
              <div class="settings-select-item">
                <label class="settings-select-item-label">
                  {{ $t('preferences.usenet-cleanup-action') }}
                </label>
                <Select v-model="cleanupMode">
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="keep-all">
                      {{ $t('preferences.usenet-cleanup-keep') }}
                    </SelectItem>
                    <SelectItem value="delete-par2">
                      {{ $t('preferences.usenet-cleanup-par2') }}
                    </SelectItem>
                    <SelectItem value="delete-par2-and-archives" disabled>
                      {{ $t('preferences.usenet-cleanup-all') }}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <p class="field-hint">{{ cleanupDescription }}</p>
              </div>
            </div>
          </div>
        </section>

        <section class="settings-section">
          <div class="settings-section-header">
            <div class="section-icon"><ShieldCheck :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.usenet-archive-safety') }}</h3>
              <p>{{ $t('preferences.usenet-limits-defaults') }}</p>
            </div>
          </div>

          <div class="settings-section-content">
            <div v-if="archiveLimitsAdjusted" class="limits-adjusted-notice">
              <TriangleAlert :size="16" />
              <div>
                <strong>{{ $t('preferences.usenet-limits-adjusted') }}</strong>
                <span>{{ $t('preferences.usenet-limits-adjusted-tips') }}</span>
              </div>
            </div>

            <div class="archive-summary-row">
              <div class="archive-summary-content">
                <span class="archive-summary-title">
                  {{ isAndroid ? $t('preferences.usenet-android-limits') : $t('preferences.usenet-desktop-limits') }}
                </span>
                <span class="archive-summary-description">{{ limitsSummary }}</span>
                <span class="archive-summary-description">{{ limitsSecondarySummary }}</span>
              </div>
              <Button
                class="archive-summary-action"
                size="sm"
                variant="outline"
                :aria-expanded="limitsExpanded"
                @click="limitsExpanded = !limitsExpanded"
              >
                <SlidersHorizontal :size="14" />
                {{ limitsExpanded ? $t('preferences.usenet-hide-limits') : $t('preferences.usenet-customize-limits') }}
              </Button>
            </div>

            <div v-if="limitsExpanded" class="archive-limits-panel">
              <div class="archive-limits-grid">
                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-entries') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="limits.maxEntries"
                      :min="1"
                      :max="limitCeilings.maxEntries"
                      :step="1000"
                      @update:model-value="(value: number) => saveLimit('maxEntries', value)"
                    />
                    <span class="limit-unit">{{ $t('preferences.usenet-unit-entries') }}</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-expanded') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="bytesToGiB(limits.maxExpandedBytes)"
                      :min="1"
                      :max="bytesToGiB(limitCeilings.maxExpandedBytes)"
                      :step="1"
                      @update:model-value="(value: number) => saveGiBLimit('maxExpandedBytes', value)"
                    />
                    <span class="limit-unit">GiB</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-entry') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="bytesToGiB(limits.maxEntryBytes)"
                      :min="1"
                      :max="bytesToGiB(limitCeilings.maxEntryBytes)"
                      :step="1"
                      @update:model-value="(value: number) => saveGiBLimit('maxEntryBytes', value)"
                    />
                    <span class="limit-unit">GiB</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-depth') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="limits.maxNestingDepth"
                      :min="1"
                      :max="limitCeilings.maxNestingDepth"
                      :step="1"
                      @update:model-value="(value: number) => saveLimit('maxNestingDepth', value)"
                    />
                    <span class="limit-unit">{{ $t('preferences.usenet-unit-levels') }}</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-ratio') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="limits.maxCompressionRatio"
                      :min="1"
                      :max="limitCeilings.maxCompressionRatio"
                      :step="10"
                      @update:model-value="(value: number) => saveLimit('maxCompressionRatio', value)"
                    />
                    <span class="limit-unit">: 1</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-free-space-reserve') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="bytesToGiB(limits.freeSpaceReserveBytes)"
                      :min="1"
                      :max="bytesToGiB(limitCeilings.freeSpaceReserveBytes)"
                      :step="1"
                      @update:model-value="(value: number) => saveGiBLimit('freeSpaceReserveBytes', value)"
                    />
                    <span class="limit-unit">GiB</span>
                  </div>
                </label>

                <label class="limit-field">
                  <span class="limit-field-label">{{ $t('preferences.usenet-max-active-time') }}</span>
                  <div class="limit-control">
                    <NumberInput
                      :model-value="secondsToHours(limits.maxActiveSeconds)"
                      :min="0.5"
                      :max="secondsToHours(limitCeilings.maxActiveSeconds)"
                      :step="0.5"
                      @update:model-value="(value: number) => saveHoursLimit(value)"
                    />
                    <span class="limit-unit">{{ $t('preferences.usenet-unit-hours') }}</span>
                  </div>
                </label>
              </div>

              <div class="archive-limits-footer">
                <p>{{ $t('preferences.usenet-limits-hard-cap') }}</p>
                <Button size="sm" variant="ghost" @click="restoreDefaultLimits">
                  <RotateCcw :size="14" />
                  {{ $t('preferences.usenet-restore-limits') }}
                </Button>
              </div>
            </div>
          </div>
        </section>
      </div>
    </main>

    <Dialog
      :open="profileDialogOpen"
      @update:open="(open: boolean) => { if (!open && !savingProfile) closeProfileDialog() }"
    >
      <DialogContent
        class="usenet-profile-dialog"
        :show-close-button="!savingProfile"
        aria-describedby="usenet-profile-description"
      >
        <DialogHeader>
          <DialogTitle>{{ profileDialogTitle }}</DialogTitle>
          <p id="usenet-profile-description" class="dialog-subtitle">
            {{ $t(preferenceStore.vaultEnabled ? 'preferences.usenet-profile-dialog-tips-vault' : 'preferences.usenet-profile-dialog-tips') }}
          </p>
        </DialogHeader>

        <form class="profile-dialog-form" @submit.prevent="saveProfile">
          <div class="dialog-field">
            <Label for="usenet-profile-name">{{ $t('preferences.usenet-profile-name') }}</Label>
            <Input
              id="usenet-profile-name"
              v-model="profileForm.name"
              :placeholder="$t('preferences.usenet-profile-name-placeholder')"
              autocomplete="off"
            />
          </div>

          <div class="profile-endpoint-grid">
            <div class="dialog-field">
              <Label for="usenet-profile-host">{{ $t('preferences.usenet-profile-host') }}</Label>
              <Input
                id="usenet-profile-host"
                v-model="profileForm.host"
                placeholder="news.example.com"
                autocomplete="off"
                autocapitalize="none"
                spellcheck="false"
              />
            </div>
            <label class="dialog-field">
              <span class="dialog-field-label">{{ $t('preferences.usenet-profile-port') }}</span>
              <NumberInput v-model="profileForm.port" :min="1" :max="65535" :step="1" />
            </label>
          </div>

          <div class="profile-dialog-grid">
            <div class="dialog-field">
              <Label for="usenet-profile-security">{{ $t('preferences.usenet-security') }}</Label>
              <Select
                :model-value="profileForm.securityMode"
                @update:model-value="changeSecurityMode"
              >
                <SelectTrigger id="usenet-profile-security" class="profile-dialog-select">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="implicit-tls">{{ $t('preferences.usenet-tls') }}</SelectItem>
                  <SelectItem value="starttls">{{ $t('preferences.usenet-starttls') }}</SelectItem>
                  <SelectItem value="plain">{{ $t('preferences.usenet-plain') }}</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div class="dialog-field">
              <Label for="usenet-profile-auth">{{ $t('preferences.usenet-authentication') }}</Label>
              <Select v-model="profileForm.authMode">
                <SelectTrigger id="usenet-profile-auth" class="profile-dialog-select">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="credentials">{{ $t('preferences.usenet-auth-credentials') }}</SelectItem>
                  <SelectItem value="anonymous">{{ $t('preferences.usenet-auth-anonymous') }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div v-if="profileForm.authMode === 'credentials'" class="profile-dialog-grid">
            <div class="dialog-field">
              <Label for="usenet-profile-username">{{ $t('preferences.usenet-username') }}</Label>
              <Input
                id="usenet-profile-username"
                v-model="profileForm.username"
                autocomplete="username"
                autocapitalize="none"
                spellcheck="false"
              />
            </div>
            <div class="dialog-field">
              <Label for="usenet-profile-password">{{ $t('preferences.usenet-password') }}</Label>
              <Input
                id="usenet-profile-password"
                v-model="profileForm.password"
                type="password"
                autocomplete="new-password"
              />
            </div>
            <p v-if="profileHasSavedCredentials" class="credential-hint">
              <KeyRound :size="13" />
              {{ $t('preferences.usenet-credentials-saved-tips') }}
            </p>
          </div>

          <div class="profile-dialog-grid">
            <label class="dialog-field">
              <span class="dialog-field-label">{{ $t('preferences.usenet-priority') }}</span>
              <NumberInput v-model="profileForm.priority" :min="0" :max="999" :step="1" />
              <span class="dialog-hint">{{ $t('preferences.usenet-priority-tips') }}</span>
            </label>
            <label class="dialog-field">
              <span class="dialog-field-label">{{ $t('preferences.usenet-max-connections') }}</span>
              <NumberInput v-model="profileForm.maxConnections" :min="1" :max="128" :step="1" />
              <span class="dialog-hint">{{ $t('preferences.usenet-max-connections-tips') }}</span>
            </label>
          </div>

          <div class="dialog-toggle-row">
            <div>
              <span class="dialog-toggle-title">{{ $t('preferences.usenet-enabled') }}</span>
              <span class="dialog-hint">{{ $t('preferences.usenet-enabled-tips') }}</span>
            </div>
            <ui-switch v-model="profileForm.enabled" />
          </div>

          <div v-if="profileForm.securityMode === 'plain'" class="plain-warning">
            <TriangleAlert :size="18" />
            <div>
              <strong>{{ $t('preferences.usenet-plain-warning-title') }}</strong>
              <p>{{ $t('preferences.usenet-plain-warning') }}</p>
								<div class="plain-confirmation">
									<ui-checkbox v-model="profileForm.allowPlain" />
									<span>{{ $t('preferences.usenet-plain-opt-in') }}</span>
								</div>
            </div>
          </div>

          <div v-if="profileFormError" class="dialog-error" role="alert">
            <CircleAlert :size="15" />
            <span>{{ profileFormError }}</span>
          </div>

          <DialogFooter class="profile-dialog-footer">
            <Button type="button" variant="outline" :disabled="savingProfile" @click="closeProfileDialog">
              {{ $t('app.cancel') }}
            </Button>
            <Button type="submit" :disabled="savingProfile">
              <Loader2 v-if="savingProfile" :size="14" class="animate-spin" />
              {{ $t('app.save') }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </div>
</template>

<script lang="ts">
import {
	Archive,
	CircleAlert,
	KeyRound,
	Loader2,
	Pencil,
	Plug,
	Plus,
	Radio,
	RotateCcw,
	Server,
	ServerOff,
	ShieldCheck,
	SlidersHorizontal,
	Trash2,
	TriangleAlert,
} from "@lucide/vue";
import {
	ANDROID_USENET_ARCHIVE_LIMITS,
	DEFAULT_USENET_ARCHIVE_LIMITS,
	type UsenetArchiveLimits,
	type UsenetCleanupMode,
	type UsenetProviderProfile,
	type UsenetSecurityMode,
} from "@shared/types/usenet";
import { defineComponent } from "vue";
import { toast } from "vue-sonner";
import api from "@/api";
import { Button } from "@/components/ui/button";
import { confirm } from "@/components/ui/confirm-dialog";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import NumberInput from "@/components/ui/NumberInput.vue";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import is from "@/shims/platform";
import { usePreferenceStore } from "@/store/preference";

const GIB = 1024 ** 3;

type AuthMode = "credentials" | "anonymous";

interface ProviderFormState {
	id: string;
	name: string;
	host: string;
	port: number;
	securityMode: UsenetSecurityMode;
	enabled: boolean;
	priority: number;
	maxConnections: number;
	allowPlain: boolean;
	authMode: AuthMode;
	username: string;
	password: string;
}

function emptyProfileForm(): ProviderFormState {
	return {
		id: crypto.randomUUID(),
		name: "",
		host: "",
		port: 563,
		securityMode: "implicit-tls",
		enabled: true,
		priority: 0,
		maxConnections: 4,
		allowPlain: false,
		authMode: "credentials",
		username: "",
		password: "",
	};
}

function standardPort(mode: UsenetSecurityMode): number {
	return mode === "implicit-tls" ? 563 : 119;
}

export default defineComponent({
	name: "preference-usenet",
	components: {
		Archive,
		Button,
		CircleAlert,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		Input,
		KeyRound,
		Label,
		Loader2,
		NumberInput,
		Pencil,
		Plug,
		Plus,
		Radio,
		RotateCcw,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Server,
		ServerOff,
		ShieldCheck,
		SlidersHorizontal,
		Trash2,
		TriangleAlert,
	},
	data() {
		return {
			profileDialogOpen: false,
			profileForm: emptyProfileForm(),
			profileFormError: "",
			savingProfile: false,
			testingId: null as string | null,
			credentialStatuses: {} as Record<string, boolean | undefined>,
			limitsExpanded: false,
		};
	},
	computed: {
		preferenceStore() {
			return usePreferenceStore();
		},
		isAndroid: () => is.android(),
		profiles(): UsenetProviderProfile[] {
			return (this.preferenceStore.config.usenetProfiles || [])
				.filter((profile) => !profile.deletedAt)
				.slice()
				.sort(
					(left, right) =>
						left.priority - right.priority ||
						left.name.localeCompare(right.name),
				);
		},
		profileIds(): string {
			return this.profiles.map((profile) => profile.id).join("\u0000");
		},
		cleanupMode: {
			get(): UsenetCleanupMode {
				return this.preferenceStore.config.usenetCleanupMode || "keep-all";
			},
			set(value: UsenetCleanupMode) {
				this.preferenceStore
					.save({ usenetCleanupMode: value })
					.catch((error: unknown) => {
						toast.error(this.errorMessage(error));
					});
			},
		},
		cleanupDescription(): string {
			const key: Record<UsenetCleanupMode, string> = {
				"keep-all": "preferences.usenet-cleanup-keep-tips",
				"delete-par2": "preferences.usenet-cleanup-par2-tips",
				"delete-par2-and-archives": "preferences.usenet-cleanup-all-tips",
			};
			return this.$t(key[this.cleanupMode]) as string;
		},
		platformDefaults(): UsenetArchiveLimits {
			return this.isAndroid
				? ANDROID_USENET_ARCHIVE_LIMITS
				: DEFAULT_USENET_ARCHIVE_LIMITS;
		},
		limits(): UsenetArchiveLimits {
			return {
				...this.platformDefaults,
				...(this.preferenceStore.config.usenetArchiveLimits || {}),
			};
		},
		limitCeilings(): UsenetArchiveLimits {
			return {
				maxEntries: this.platformDefaults.maxEntries * 4,
				maxExpandedBytes: this.platformDefaults.maxExpandedBytes * 4,
				maxEntryBytes: this.platformDefaults.maxEntryBytes * 4,
				maxNestingDepth: this.platformDefaults.maxNestingDepth * 4,
				maxCompressionRatio: this.platformDefaults.maxCompressionRatio * 4,
				freeSpaceReserveBytes: this.platformDefaults.freeSpaceReserveBytes * 4,
				maxActiveSeconds: this.platformDefaults.maxActiveSeconds * 4,
			};
		},
		archiveLimitsAdjusted(): boolean {
			return !!this.preferenceStore.config.usenetLimitsAdjusted;
		},
		limitsSummary(): string {
			return this.$t("preferences.usenet-limits-summary", {
				entries: new Intl.NumberFormat().format(this.limits.maxEntries),
				total: this.formatBinaryBytes(this.limits.maxExpandedBytes),
				perEntry: this.formatBinaryBytes(this.limits.maxEntryBytes),
			}) as string;
		},
		limitsSecondarySummary(): string {
			return this.$t("preferences.usenet-limits-secondary-summary", {
				ratio: new Intl.NumberFormat().format(this.limits.maxCompressionRatio),
				depth: this.limits.maxNestingDepth,
				reserve: this.formatBinaryBytes(this.limits.freeSpaceReserveBytes),
				hours: this.secondsToHours(this.limits.maxActiveSeconds),
			}) as string;
		},
		profileDialogTitle(): string {
			const exists = (this.preferenceStore.config.usenetProfiles || []).some(
				(profile) => profile.id === this.profileForm.id && !profile.deletedAt,
			);
			return this.$t(
				exists
					? "preferences.usenet-edit-profile"
					: "preferences.usenet-add-profile",
			) as string;
		},
		profileHasSavedCredentials(): boolean {
			return !!this.credentialStatuses[this.profileForm.id];
		},
	},
	created() {
		if (this.archiveLimitsAdjusted) {
			this.limitsExpanded = true;
		}
	},
	watch: {
		profileIds: {
			handler() {
				this.refreshCredentialStatuses();
			},
			immediate: true,
		},
	},
	methods: {
		errorMessage(error: unknown): string {
			return error instanceof Error ? error.message : String(error);
		},
		securityLabel(mode: UsenetSecurityMode): string {
			const key: Record<UsenetSecurityMode, string> = {
				"implicit-tls": "preferences.usenet-tls-short",
				starttls: "preferences.usenet-starttls",
				plain: "preferences.usenet-plain",
			};
			return this.$t(key[mode]) as string;
		},
		credentialLabel(profileId: string): string {
			if (this.credentialStatuses[profileId] === undefined) {
				return this.$t("preferences.usenet-credentials-unavailable") as string;
			}
			return this.$t(
				this.credentialStatuses[profileId]
					? "preferences.usenet-credentials-saved"
					: "preferences.usenet-auth-anonymous",
			) as string;
		},
		async refreshCredentialStatuses() {
			await Promise.all(
				this.profiles.map(async (profile) => {
					try {
						this.credentialStatuses[profile.id] =
							await api.hasUsenetCredentials(profile.id);
					} catch {
						this.credentialStatuses[profile.id] = undefined;
					}
				}),
			);
		},
		async restoreProfiles(
			storedProfiles: UsenetProviderProfile[],
			originalError: unknown,
		): Promise<never> {
			try {
				await this.preferenceStore.save({ usenetProfiles: storedProfiles });
			} catch (rollbackError) {
				throw new Error(
					`${this.errorMessage(originalError)} (profile rollback failed: ${this.errorMessage(rollbackError)})`,
				);
			}
			throw originalError;
		},
		openCreateProfile() {
			this.profileForm = emptyProfileForm();
			this.profileFormError = "";
			this.profileDialogOpen = true;
		},
		openEditProfile(profile: UsenetProviderProfile) {
			this.profileForm = {
				id: profile.id,
				name: profile.name,
				host: profile.host,
				port: profile.port,
				securityMode: profile.securityMode,
				enabled: profile.enabled,
				priority: profile.priority,
				maxConnections: profile.maxConnections,
				allowPlain: profile.allowPlain,
				authMode:
					this.credentialStatuses[profile.id] === false
						? "anonymous"
						: "credentials",
				username: "",
				password: "",
			};
			this.profileFormError = "";
			this.profileDialogOpen = true;
		},
		closeProfileDialog() {
			if (this.savingProfile) {
				return;
			}
			this.profileDialogOpen = false;
			this.profileFormError = "";
		},
		changeSecurityMode(value: unknown) {
			if (
				value !== "implicit-tls" &&
				value !== "starttls" &&
				value !== "plain"
			) {
				return;
			}
			const previousMode = this.profileForm.securityMode;
			if (this.profileForm.port === standardPort(previousMode)) {
				this.profileForm.port = standardPort(value);
			}
			this.profileForm.securityMode = value;
			if (value !== "plain") {
				this.profileForm.allowPlain = false;
			}
		},
		validateProfileForm(): string {
			if (!this.profileForm.name.trim()) {
				return this.$t("preferences.usenet-validation-name") as string;
			}
			if (!this.profileForm.host.trim()) {
				return this.$t("preferences.usenet-validation-host") as string;
			}
			if (
				!Number.isInteger(this.profileForm.port) ||
				this.profileForm.port < 1 ||
				this.profileForm.port > 65535
			) {
				return this.$t("preferences.usenet-validation-port") as string;
			}
			if (
				this.profileForm.securityMode === "plain" &&
				!this.profileForm.allowPlain
			) {
				return this.$t("preferences.usenet-validation-plain") as string;
			}
			if (
				!Number.isInteger(this.profileForm.priority) ||
				this.profileForm.priority < 0
			) {
				return this.$t("preferences.usenet-validation-priority") as string;
			}
			if (
				!Number.isInteger(this.profileForm.maxConnections) ||
				this.profileForm.maxConnections < 1 ||
				this.profileForm.maxConnections > 128
			) {
				return this.$t("preferences.usenet-validation-connections") as string;
			}
			if (this.profileForm.authMode === "credentials") {
				const hasUsername = !!this.profileForm.username.trim();
				const hasPassword = this.profileForm.password.length > 0;
				if (hasUsername !== hasPassword) {
					return this.$t("preferences.usenet-validation-credentials") as string;
				}
				if (!hasUsername && !this.profileHasSavedCredentials) {
					const isExistingProfile = (
						this.preferenceStore.config.usenetProfiles || []
					).some(
						(profile) =>
							profile.id === this.profileForm.id && !profile.deletedAt,
					);
					if (
						isExistingProfile &&
						this.credentialStatuses[this.profileForm.id] === undefined
					) {
						return this.$t(
							"preferences.usenet-validation-credential-status",
						) as string;
					}
					return this.$t("preferences.usenet-validation-credentials") as string;
				}
			}
			return "";
		},
		async saveProfile() {
			this.profileFormError = this.validateProfileForm();
			if (this.profileFormError) {
				return;
			}

			this.savingProfile = true;
			const now = Date.now();
			const profile: UsenetProviderProfile = {
				id: this.profileForm.id,
				name: this.profileForm.name.trim(),
				host: this.profileForm.host.trim(),
				port: this.profileForm.port,
				securityMode: this.profileForm.securityMode,
				enabled: this.profileForm.enabled,
				priority: this.profileForm.priority,
				maxConnections: this.profileForm.maxConnections,
				allowPlain:
					this.profileForm.securityMode === "plain" &&
					this.profileForm.allowPlain,
				updatedAt: now,
			};

			const storedProfiles = [
				...(this.preferenceStore.config.usenetProfiles || []),
			];
			const existingIndex = storedProfiles.findIndex(
				(item) => item.id === profile.id,
			);
			const nextProfiles = [...storedProfiles];
			if (existingIndex >= 0) {
				nextProfiles.splice(existingIndex, 1, profile);
			} else {
				nextProfiles.push(profile);
			}

			try {
				await this.preferenceStore.save({ usenetProfiles: nextProfiles });
				if (this.profileForm.authMode === "anonymous") {
					await api.removeUsenetCredentials(profile.id);
				} else if (
					this.profileForm.username.trim() &&
					this.profileForm.password
				) {
					await api.saveUsenetCredentials(
						profile.id,
						this.profileForm.username.trim(),
						this.profileForm.password,
					);
				}

				this.credentialStatuses[profile.id] =
					this.profileForm.authMode !== "anonymous";
				this.profileDialogOpen = false;
				toast.success(this.$t("preferences.usenet-profile-saved") as string);
			} catch (error) {
				let reportedError: unknown = error;
				try {
					await this.restoreProfiles(storedProfiles, error);
				} catch (rollbackError) {
					reportedError = rollbackError;
				}
				this.profileFormError = this.errorMessage(reportedError);
			} finally {
				this.savingProfile = false;
			}
		},
		async setProfileEnabled(profile: UsenetProviderProfile, enabled: boolean) {
			const now = Date.now();
			const nextProfiles = (
				this.preferenceStore.config.usenetProfiles || []
			).map((item) =>
				item.id === profile.id ? { ...item, enabled, updatedAt: now } : item,
			);
			try {
				await this.preferenceStore.save({ usenetProfiles: nextProfiles });
			} catch (error) {
				toast.error(this.errorMessage(error));
			}
		},
		async testProfile(profile: UsenetProviderProfile) {
			this.testingId = profile.id;
			try {
				await api.testUsenetProfile(profile);
				toast.success(
					this.$t("preferences.usenet-test-success", {
						name: profile.name,
					}) as string,
				);
			} catch (error) {
				toast.error(
					this.$t("preferences.usenet-test-failed", {
						message: this.errorMessage(error),
					}) as string,
				);
			} finally {
				this.testingId = null;
			}
		},
		async removeProfile(profile: UsenetProviderProfile) {
			const { confirmed } = await confirm({
				title: this.$t("preferences.usenet-remove-title") as string,
				message: this.$t("preferences.usenet-remove-confirm", {
					name: profile.name,
				}) as string,
				kind: "warning",
				confirmText: this.$t("preferences.usenet-remove") as string,
				cancelText: this.$t("app.cancel") as string,
			});
			if (!confirmed) {
				return;
			}

			const now = Date.now();
			const storedProfiles = [
				...(this.preferenceStore.config.usenetProfiles || []),
			];
			const nextProfiles = storedProfiles.map((item) =>
				item.id === profile.id
					? { ...item, enabled: false, deletedAt: now, updatedAt: now }
					: item,
			);
			try {
				await this.preferenceStore.save({ usenetProfiles: nextProfiles });
				await api.removeUsenetCredentials(profile.id);
				delete this.credentialStatuses[profile.id];
				toast.success(
					this.$t("preferences.usenet-profile-removed", {
						name: profile.name,
					}) as string,
				);
			} catch (error) {
				let reportedError: unknown = error;
				try {
					await this.restoreProfiles(storedProfiles, error);
				} catch (rollbackError) {
					reportedError = rollbackError;
				}
				toast.error(this.errorMessage(reportedError));
			}
		},
		bytesToGiB(bytes: number): number {
			return Math.round((bytes / GIB) * 100) / 100;
		},
		formatBinaryBytes(bytes: number): string {
			const units = [
				[1024 ** 4, "TiB"],
				[GIB, "GiB"],
				[1024 ** 2, "MiB"],
			] as const;
			for (const [divisor, unit] of units) {
				if (bytes >= divisor) {
					const amount = bytes / divisor;
					return `${Number.isInteger(amount) ? amount : amount.toFixed(1)} ${unit}`;
				}
			}
			return `${bytes} B`;
		},
		secondsToHours(seconds: number): number {
			return Math.round((seconds / 3600) * 100) / 100;
		},
		saveArchiveLimits(limits: UsenetArchiveLimits) {
			this.preferenceStore
				.save({ usenetArchiveLimits: limits, usenetLimitsAdjusted: false })
				.catch((error: unknown) => toast.error(this.errorMessage(error)));
		},
		saveLimit(key: keyof UsenetArchiveLimits, value: number) {
			const nextValue = Math.min(
				this.limitCeilings[key],
				Math.max(1, Math.floor(value)),
			);
			this.saveArchiveLimits({ ...this.limits, [key]: nextValue });
		},
		saveGiBLimit(
			key: "maxExpandedBytes" | "maxEntryBytes" | "freeSpaceReserveBytes",
			value: number,
		) {
			const nextValue = Math.min(
				this.limitCeilings[key],
				Math.max(GIB, Math.round(value * GIB)),
			);
			this.saveArchiveLimits({ ...this.limits, [key]: nextValue });
		},
		saveHoursLimit(value: number) {
			const seconds = Math.min(
				this.limitCeilings.maxActiveSeconds,
				Math.max(30 * 60, Math.round(value * 3600)),
			);
			this.saveArchiveLimits({ ...this.limits, maxActiveSeconds: seconds });
		},
		async restoreDefaultLimits() {
			try {
				await this.preferenceStore.save({
					usenetArchiveLimits: { ...this.platformDefaults },
					usenetLimitsAdjusted: false,
				});
				toast.success(this.$t("preferences.usenet-limits-restored") as string);
			} catch (error) {
				toast.error(this.errorMessage(error));
			}
		},
	},
});
</script>

<style scoped>
.usenet-section-header {
	align-items: flex-start;
	flex-wrap: wrap;
}

.section-add-button {
	margin-left: auto;
	flex-shrink: 0;
}

.provider-list {
	display: flex;
	flex-direction: column;
	gap: 8px;
}

.provider-card {
	display: grid;
	grid-template-columns: 36px minmax(0, 1fr) auto;
	align-items: center;
	gap: 12px;
	padding: 12px 14px;
	border: 1px solid var(--border);
	border-radius: var(--radius);
	background: var(--surface-1);
	transition:
		border-color var(--dur-2) var(--ease-out),
		background-color var(--dur-2) var(--ease-out),
		opacity var(--dur-2) var(--ease-out);
}

.provider-card:hover {
	border-color: var(--border-strong);
	background: color-mix(in srgb, var(--surface-1) 72%, var(--surface-2));
}

.provider-card--disabled {
	opacity: 0.76;
}

.provider-card-icon {
	display: flex;
	align-items: center;
	justify-content: center;
	width: 36px;
	height: 36px;
	border-radius: calc(var(--radius) - 2px);
	background: var(--surface-2);
	color: var(--text-2);
}

.provider-card-main {
	min-width: 0;
}

.provider-card-heading,
.provider-card-details,
.provider-card-actions,
.provider-enable-control,
.provider-credential-state {
	display: flex;
	align-items: center;
}

.provider-card-heading {
	gap: 6px;
	flex-wrap: wrap;
}

.provider-card-name {
	min-width: 0;
	font-size: 13px;
	font-weight: 600;
	color: var(--text-1);
	overflow-wrap: anywhere;
}

.provider-badge {
	display: inline-flex;
	align-items: center;
	padding: 2px 6px;
	border-radius: calc(var(--radius) - 4px);
	background: var(--primary-soft);
	color: var(--primary);
	font-size: 10px;
	font-weight: 600;
	line-height: 1.3;
}

.provider-badge--muted {
	background: var(--surface-2);
	color: var(--text-2);
}

.provider-endpoint {
	margin-top: 3px;
	color: var(--text-2);
	font-family: var(--app-font-family-mono);
	font-size: 12px;
	overflow: hidden;
	text-overflow: ellipsis;
	white-space: nowrap;
}

.provider-card-details {
	gap: 6px 12px;
	margin-top: 5px;
	color: var(--text-3);
	font-size: 11px;
	font-variant-numeric: tabular-nums;
	flex-wrap: wrap;
}

.provider-credential-state {
	gap: 4px;
}

.provider-card-actions {
	gap: 3px;
	flex-shrink: 0;
}

.provider-enable-control {
	gap: 7px;
	padding-right: 9px;
	margin-right: 3px;
	border-right: 1px solid var(--border);
	color: var(--text-2);
	font-size: 11px;
}

.provider-delete-button {
	color: var(--danger);
}

.provider-delete-button:hover {
	color: var(--danger);
	background: color-mix(in srgb, var(--danger) 10%, transparent);
}

.provider-empty-state {
	display: flex;
	flex-direction: column;
	align-items: center;
	justify-content: center;
	gap: 7px;
	padding: 32px 16px;
	border: 1px dashed var(--border-strong);
	border-radius: var(--radius);
	text-align: center;
}

.provider-empty-icon {
	margin-bottom: 2px;
	color: var(--text-3);
}

.provider-empty-title {
	margin: 0;
	color: var(--text-1);
	font-size: 13px;
	font-weight: 600;
}

.provider-empty-hint {
	max-width: 42ch;
	margin: 0 0 5px;
	color: var(--text-2);
	font-size: 12px;
	line-height: 1.45;
}

.cleanup-select-group {
	padding-top: 4px;
}

.field-hint,
.dialog-hint,
.credential-hint,
.archive-limits-footer p {
	margin: 0;
	color: var(--text-2);
	font-size: 11px;
	line-height: 1.45;
}

.limits-adjusted-notice,
.plain-warning,
.dialog-error {
	display: flex;
	align-items: flex-start;
	gap: 9px;
	border-radius: calc(var(--radius) - 2px);
}

.limits-adjusted-notice {
	margin-bottom: 10px;
	padding: 10px 12px;
	border: 1px solid color-mix(in srgb, var(--warning) 32%, var(--border));
	background: color-mix(in srgb, var(--warning) 9%, transparent);
	color: var(--warning);
}

.limits-adjusted-notice > div,
.plain-warning > div {
	display: flex;
	flex: 1;
	min-width: 0;
	flex-direction: column;
	gap: 2px;
}

.limits-adjusted-notice strong,
.plain-warning strong {
	font-size: 12px;
	font-weight: 600;
}

.limits-adjusted-notice span {
	color: var(--text-2);
	font-size: 11px;
	line-height: 1.4;
}

.archive-summary-row {
	display: flex;
	align-items: center;
	gap: 16px;
	min-height: 68px;
	padding: 10px 0 14px;
}

.archive-summary-content {
	display: flex;
	flex: 1;
	min-width: 0;
	flex-direction: column;
	gap: 3px;
}

.archive-summary-title {
	color: var(--text-1);
	font-size: 13px;
	font-weight: 600;
}

.archive-summary-description {
	color: var(--text-2);
	font-size: 11.5px;
	line-height: 1.4;
	overflow-wrap: anywhere;
}

.archive-summary-action {
	flex-shrink: 0;
}

.archive-limits-panel {
	padding-top: 14px;
	border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
}

.archive-limits-grid {
	display: grid;
	grid-template-columns: repeat(2, minmax(0, 1fr));
	gap: 14px 16px;
}

.limit-field,
.dialog-field {
	display: flex;
	min-width: 0;
	flex-direction: column;
	gap: 6px;
}

.limit-field-label,
.dialog-field-label {
	color: var(--text-2);
	font-size: 12px;
	font-weight: 500;
}

.limit-control {
	display: grid;
	grid-template-columns: minmax(0, 1fr) auto;
	align-items: center;
	gap: 8px;
}

.limit-unit {
	min-width: 42px;
	color: var(--text-3);
	font-size: 11px;
	font-variant-numeric: tabular-nums;
}

.archive-limits-footer {
	display: flex;
	align-items: center;
	justify-content: space-between;
	gap: 12px;
	margin-top: 16px;
	padding-top: 12px;
	border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
}

.usenet-profile-dialog {
	width: calc(100vw - 32px);
	max-width: 640px;
	max-height: calc(100dvh - 32px);
}

.dialog-subtitle {
	margin: 3px 24px 0 0;
	color: var(--text-2);
	font-size: 12px;
	line-height: 1.45;
}

.profile-dialog-form {
	display: flex;
	min-height: 0;
	max-height: calc(100dvh - 150px);
	flex-direction: column;
	gap: 13px;
	overflow-y: auto;
	padding: 2px 3px 1px 0;
	scrollbar-width: thin;
}

.profile-dialog-grid,
.profile-endpoint-grid {
	display: grid;
	grid-template-columns: repeat(2, minmax(0, 1fr));
	gap: 12px;
}

.profile-dialog-select {
	width: 100%;
	min-width: 0;
	max-width: 100%;
}

.profile-dialog-select :deep([data-slot="select-value"]) {
	min-width: 0;
	overflow: hidden;
	text-overflow: ellipsis;
}

.profile-endpoint-grid {
	grid-template-columns: minmax(0, 1fr) 112px;
}

.credential-hint {
	display: flex;
	grid-column: 1 / -1;
	align-items: center;
	gap: 5px;
	margin-top: -4px;
}

.dialog-toggle-row {
	display: flex;
	align-items: center;
	justify-content: space-between;
	gap: 14px;
	min-height: 52px;
	padding: 9px 11px;
	border: 1px solid var(--border);
	border-radius: calc(var(--radius) - 2px);
	background: color-mix(in srgb, var(--surface-2) 42%, transparent);
}

.dialog-toggle-row > div {
	display: flex;
	min-width: 0;
	flex-direction: column;
	gap: 2px;
}

.dialog-toggle-title {
	color: var(--text-1);
	font-size: 13px;
	font-weight: 500;
}

.plain-warning {
	padding: 11px 12px;
	border: 1px solid color-mix(in srgb, var(--warning) 36%, var(--border));
	background: color-mix(in srgb, var(--warning) 9%, transparent);
	color: var(--warning);
}

.plain-warning p {
	margin: 0;
	color: var(--text-2);
	font-size: 11px;
	line-height: 1.45;
}

	.plain-confirmation {
	display: flex;
	align-items: center;
	gap: 8px;
	margin-top: 7px;
	color: var(--text-1);
	font-size: 12px;
	cursor: pointer;
}

.dialog-error {
	padding: 9px 10px;
	border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--border));
	background: color-mix(in srgb, var(--danger) 9%, transparent);
	color: var(--danger);
	font-size: 12px;
	line-height: 1.4;
}

.profile-dialog-footer {
	position: sticky;
	bottom: 0;
	margin-top: 2px;
	padding-top: 12px;
	border-top: 1px solid var(--border);
	background: var(--bg);
}

:global(html.platform-android .provider-list) {
	gap: 0;
}

:global(html.platform-android .provider-card) {
	padding: 12px 0;
	border: 0;
	border-radius: 0;
	background: transparent;
}

:global(html.platform-android .provider-card + .provider-card) {
	border-top: 1px solid var(--border);
}

:global(html.platform-android .provider-card:hover) {
	background: transparent;
}

@media (max-width: 640px) {
	.provider-card {
		grid-template-columns: 36px minmax(0, 1fr);
		padding: 12px;
	}

	.provider-card-actions {
		grid-column: 1 / -1;
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto auto;
		width: 100%;
		margin-top: 2px;
		padding-top: 10px;
		border-top: 1px solid var(--border);
	}

	.provider-test-button {
		width: 100%;
	}

	.provider-enable-control {
		padding-right: 8px;
	}

	.archive-summary-row,
	.archive-limits-footer {
		align-items: stretch;
		flex-direction: column;
	}

	.archive-summary-action {
		width: 100%;
	}

	.archive-limits-grid,
	.profile-dialog-grid,
	.profile-endpoint-grid {
		grid-template-columns: minmax(0, 1fr);
	}

	.profile-dialog-form {
		max-height: calc(100dvh - 132px);
	}

	.profile-dialog-footer :deep(button) {
		width: 100%;
	}
}

@media (max-width: 420px) {
	.usenet-section-header .section-title {
		min-width: calc(100% - 40px);
	}

	.section-add-button {
		width: 100%;
		margin-left: 40px;
	}

	.provider-card-details {
		column-gap: 8px;
	}
}
</style>
