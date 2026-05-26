<template>
  <div class="credential-manager">
    <div v-if="credentials.length === 0" class="credential-manager-empty">
      {{ $t('preferences.no-saved-credentials') }}
    </div>
    <ul v-else class="credential-manager-list">
      <li
        v-for="cred in credentials"
        :key="cred.id"
        class="credential-manager-item"
      >
        <div class="credential-manager-item-info">
          <span class="credential-manager-item-name">
            {{ cred.label || cred.host || 'Credential' }}
          </span>
          <span v-if="cred.protocol" class="credential-manager-item-protocol">
            {{ cred.protocol }}
          </span>
          <span
            v-if="cred.vaulted"
            class="credential-manager-item-vaulted"
            :title="$t('preferences.credential-encrypted-badge')"
          >
            <ShieldCheck :size="11" />
          </span>
          <span v-if="cred.host && cred.label" class="credential-manager-item-host">
            {{ cred.host }}
          </span>
        </div>
        <div class="credential-manager-item-meta">
          <span v-if="cred.lastUsedAt" class="credential-manager-item-date">
            {{ formatDate(cred.lastUsedAt) }}
          </span>
          <button
            type="button"
            class="credential-manager-item-delete"
            :title="$t('preferences.credential-delete')"
            @click="handleDelete(cred)"
          >
            <Trash2 :size="14" />
          </button>
        </div>
      </li>
    </ul>
    <div
      class="credential-manager-status"
      :class="{ 'credential-manager-status--off': !vaultEnabled }"
      :title="vaultEnabled ? '' : $t('preferences.credential-vault-unavailable')"
    >
      <ShieldCheck v-if="vaultEnabled" :size="12" />
      <ShieldOff v-else :size="12" />
      <span>
        {{ vaultEnabled ? $t('preferences.vault-enabled') : $t('preferences.vault-disabled') }}
      </span>
    </div>
  </div>
</template>

<script lang="ts">
import { ShieldCheck, ShieldOff, Trash2 } from "@lucide/vue";
import type { SavedCredential } from "@shared/types/credential";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "mo-credential-manager",
	components: { ShieldCheck, ShieldOff, Trash2 },
	computed: {
		credentials(): SavedCredential[] {
			return usePreferenceStore().getSavedCredentials();
		},
		vaultEnabled(): boolean {
			return usePreferenceStore().vaultEnabled;
		},
	},
	methods: {
		formatDate(timestamp: number): string {
			if (!timestamp) {
				return "";
			}
			return new Date(timestamp).toLocaleDateString();
		},
		handleDelete(cred: SavedCredential) {
			usePreferenceStore().removeCredential(cred.id);
		},
	},
};
</script>
