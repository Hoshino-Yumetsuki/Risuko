<template>
  <Teleport to="body">
    <div v-if="showGate" class="legal-gate">
      <div class="legal-gate-card">
        <div class="legal-gate-title">{{ $t('app.legal-title') }}</div>
        <p class="legal-gate-body">{{ $t('app.legal-body') }}</p>
        <div class="legal-gate-links">
          <button type="button" class="legal-link" @click="openUrl('https://risuko.app/legal/terms')">
            {{ $t('app.legal-terms') }}
            <ExternalLink :size="13" />
          </button>
          <button type="button" class="legal-link" @click="openUrl('https://risuko.app/legal/privacy')">
            {{ $t('app.legal-privacy') }}
            <ExternalLink :size="13" />
          </button>
        </div>
        <button type="button" class="legal-gate-agree" @click="accept">
          {{ $t('app.legal-agree') }}
        </button>
        <p class="legal-gate-note">{{ $t('app.legal-agree-note') }}</p>
      </div>
    </div>
  </Teleport>
</template>

<script>
import { ExternalLink } from "@lucide/vue";
import { invoke } from "@tauri-apps/api/core";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "legal-gate",
	components: { ExternalLink },
	computed: {
		showGate() {
			const config = usePreferenceStore().config;
			return !!config && "legalAccepted" in config && !config.legalAccepted;
		},
	},
	methods: {
		openUrl(url) {
			invoke("plugin:shell|open", { path: url }).catch(() => {});
		},
		accept() {
			usePreferenceStore()
				.save({ legalAccepted: true })
				.catch(() => {});
		},
	},
};
</script>

<style scoped>
.legal-gate {
	position: fixed;
	inset: 0;
	z-index: 9999;
	display: flex;
	align-items: center;
	justify-content: center;
	padding: 24px;
	background: color-mix(in srgb, var(--bg) 68%, transparent);
	backdrop-filter: blur(6px);
}

.legal-gate-card {
	width: 100%;
	max-width: 440px;
	padding: 28px;
	border-radius: calc(var(--radius) + 4px);
	border: 1px solid var(--border);
	background: var(--bg);
	color: var(--text-1);
	box-shadow: 0 24px 64px -16px rgba(0, 0, 0, 0.4);
}

.legal-gate-title {
	font-size: 20px;
	font-weight: 700;
	letter-spacing: -0.01em;
}

.legal-gate-body {
	margin-top: 10px;
	font-size: 14px;
	line-height: 1.6;
	color: var(--text-2);
}

.legal-gate-links {
	display: flex;
	flex-wrap: wrap;
	gap: 10px;
	margin-top: 18px;
}

.legal-link {
	display: inline-flex;
	align-items: center;
	gap: 6px;
	padding: 8px 14px;
	border-radius: calc(var(--radius) - 2px);
	border: 1px solid var(--border);
	background: var(--surface-2);
	color: var(--text-1);
	font-size: 13px;
	font-weight: 500;
	cursor: pointer;
	transition: background 0.12s ease;
}

.legal-link:hover {
	background: color-mix(in srgb, var(--surface-2) 88%, var(--text-1));
}

.legal-gate-agree {
	width: 100%;
	margin-top: 22px;
	padding: 12px;
	border: none;
	border-radius: calc(var(--radius) - 2px);
	background: var(--primary);
	color: var(--primary-foreground);
	font-size: 14px;
	font-weight: 600;
	cursor: pointer;
	transition: background 0.12s ease;
}

.legal-gate-agree:hover {
	background: var(--primary-hover);
}

.legal-gate-note {
	margin-top: 12px;
	text-align: center;
	font-size: 12px;
	color: var(--text-3);
}
</style>
