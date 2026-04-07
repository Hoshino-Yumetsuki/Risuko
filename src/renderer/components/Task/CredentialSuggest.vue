<template>
  <div v-if="hasItems" class="credential-suggest">
    <div class="credential-suggest-bar">
      <KeyRound :size="12" class="credential-suggest-icon" />
      <span v-if="matchedCredentials.length > 0" class="credential-suggest-text">
        {{ $t('task.saved-credentials-found', { host: currentHost }) }}
      </span>
      <span v-else class="credential-suggest-text">
        {{ $t('task.saved-credentials-profiles') }}
      </span>

      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <button type="button" class="credential-suggest-btn credential-suggest-btn--apply">
            <ChevronDown :size="12" />
            {{ $t('task.apply-credential') }}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent :style="{ minWidth: '240px' }" align="end">
          <template v-if="matchedCredentials.length > 0">
            <DropdownMenuLabel>{{ currentHost }}</DropdownMenuLabel>
            <DropdownMenuItem
              v-for="cred in matchedCredentials"
              :key="cred.id"
              @click="handleApply(cred)"
            >
              <span class="credential-item-label">
                {{ credentialDisplayName(cred) }}
              </span>
              <span class="credential-item-meta">{{ cred.protocol }}</span>
            </DropdownMenuItem>
          </template>
          <template v-if="matchedCredentials.length > 0 && profileCredentials.length > 0">
            <DropdownMenuSeparator />
          </template>
          <template v-if="profileCredentials.length > 0">
            <DropdownMenuLabel>{{ $t('task.saved-credentials-profiles') }}</DropdownMenuLabel>
            <DropdownMenuItem
              v-for="cred in profileCredentials"
              :key="cred.id"
              @click="handleApply(cred)"
            >
              <span class="credential-item-label">
                {{ cred.label }}
              </span>
              <span v-if="cred.protocol" class="credential-item-meta">{{ cred.protocol }}</span>
            </DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  </div>
</template>

<script lang="ts">
import type { SavedCredential } from "@shared/types/credential";
import { ChevronDown, KeyRound } from "lucide-vue-next";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuLabel,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { usePreferenceStore } from "@/store/preference";
import {
	applyCredentialToForm,
	extractHostFromUri,
	extractProtocolFromUri,
} from "@/utils/task";

export default {
	name: "mo-credential-suggest",
	components: {
		KeyRound,
		ChevronDown,
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuLabel,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
	},
	props: {
		uris: {
			type: String,
			default: "",
		},
		form: {
			type: Object,
			required: true,
		},
	},
	computed: {
		currentHost(): string | null {
			return extractHostFromUri(this.uris);
		},
		currentProtocol(): string | undefined {
			return extractProtocolFromUri(this.uris);
		},
		matchedCredentials(): SavedCredential[] {
			const host = this.currentHost;
			if (!host) {
				return [];
			}
			return usePreferenceStore().findCredentialsByHost(
				host,
				this.currentProtocol,
			);
		},
		profileCredentials(): SavedCredential[] {
			const all = usePreferenceStore().getSavedCredentials();
			const matchedIds = new Set(this.matchedCredentials.map((c) => c.id));
			return all.filter(
				(c: SavedCredential) => c.label && !matchedIds.has(c.id),
			);
		},
		hasItems(): boolean {
			return (
				this.matchedCredentials.length > 0 || this.profileCredentials.length > 0
			);
		},
	},
	methods: {
		handleApply(credential: SavedCredential) {
			applyCredentialToForm(this.form, credential);
			usePreferenceStore().updateCredentialLastUsed(credential.id);
			this.$msg.success({
				message: this.$t("task.credential-applied"),
				duration: 2000,
			});
		},
		credentialDisplayName(cred: SavedCredential): string {
			if (cred.label) {
				return cred.label;
			}
			if (cred.ftpUser) {
				return cred.ftpUser;
			}
			if (cred.authorization) {
				const preview = cred.authorization.slice(0, 20);
				return preview + (cred.authorization.length > 20 ? "..." : "");
			}
			return cred.host || "Credential";
		},
	},
};
</script>
