import { getLanguage } from "@shared/locales";
import logger from "@shared/utils/logger";
import axios from "axios";
import { defineStore } from "pinia";
import api from "@/api";
import { usePreferenceStore } from "./preference";

const syncAxios = axios.create({ timeout: 10_000 });

interface SyncUser {
	id: string;
	email: string | null;
	githubUsername: string | null;
}

interface AuthState {
	user: SyncUser | null;
	token: string | null;
	isLoggedIn: boolean;
	loading: boolean;
	serverConfig: { email: boolean; github: boolean } | null;
}

export const useAuthStore = defineStore("auth", {
	state: (): AuthState => ({
		user: null,
		token: null,
		isLoggedIn: false,
		loading: false,
		serverConfig: null,
	}),
	getters: {
		serverUrl(): string {
			return (
				(api.config?.cloudSyncServerUrl as string) || "https://api.risuko.app"
			);
		},
	},
	actions: {
		getAuthHeaders() {
			return this.token ? { Authorization: `Bearer ${this.token}` } : {};
		},

		async fetchServerConfig(): Promise<void> {
			try {
				const { data } = await syncAxios.get(`${this.serverUrl}/config`);
				this.serverConfig = data.authMethods;
			} catch (err) {
				logger.warn(
					"[Risuko] fetchServerConfig failed:",
					(err as Error).message,
				);
				this.serverConfig = null;
			}
		},

		async requestOTP(email: string): Promise<void> {
			try {
				const preferenceStore = usePreferenceStore();
				const locale = getLanguage(preferenceStore.config.locale || "auto");
				await syncAxios.post(`${this.serverUrl}/auth/magic-link`, {
					email,
					locale,
				});
			} catch (err) {
				throw new Error(
					(err as { response?: { data?: { error?: string } } }).response?.data
						?.error || "Failed to send verification code",
				);
			}
		},

		async verifyOTP(email: string, code: string): Promise<void> {
			try {
				const { data } = await syncAxios.post(
					`${this.serverUrl}/auth/verify-otp`,
					{
						email,
						code,
					},
				);
				this.token = data.token;
				this.user = data.user;
				this.isLoggedIn = true;
				await this.persistAuth();
			} catch (err) {
				throw new Error(
					(err as { response?: { data?: { error?: string } } }).response?.data
						?.error || "Verification failed",
				);
			}
		},

		async loginWithGitHub(): Promise<void> {
			const url = `${this.serverUrl}/auth/github`;
			const { invoke } = await import("@tauri-apps/api/core");
			try {
				await invoke("plugin:shell|open", { path: url });
			} catch {
				window.open(url, "_blank");
			}
		},

		async handleDeepLinkToken(token: string): Promise<void> {
			logger.info(
				"[Risuko] handleDeepLinkToken: token received, fetching user",
			);
			this.token = token;
			this.isLoggedIn = true;
			const ok = await this.fetchMe();
			if (ok) {
				logger.info("[Risuko] handleDeepLinkToken: user logged in", this.user);
				await this.persistAuth();
			} else {
				logger.warn(
					"[Risuko] handleDeepLinkToken: fetchMe failed, clearing auth",
				);
				this.clearAuth();
			}
		},

		async fetchMe(): Promise<boolean> {
			if (!this.token) {
				return false;
			}
			try {
				const { data } = await syncAxios.get(`${this.serverUrl}/auth/me`, {
					headers: this.getAuthHeaders(),
				});
				this.user = data;
				this.isLoggedIn = true;
				return true;
			} catch (err) {
				const status = (err as { response?: { status?: number } }).response
					?.status;
				if (status === 401 || status === 403) {
					logger.warn("[Risuko] fetchMe failed: token invalid");
					this.clearAuth();
					await this.persistAuth();
				} else {
					logger.warn(
						"[Risuko] fetchMe failed (network):",
						(err as Error).message,
					);
				}
				return false;
			}
		},

		async logout(): Promise<void> {
			if (this.token) {
				try {
					await syncAxios.post(
						`${this.serverUrl}/auth/logout`,
						{},
						{ headers: this.getAuthHeaders() },
					);
				} catch (err) {
					logger.warn("[Risuko] logout failed:", (err as Error).message);
				}
			}
			this.clearAuth();
			await this.persistAuth();
		},

		async persistAuth(): Promise<void> {
			await api.savePreference({
				cloudSyncToken: this.token ?? null,
			});
		},

		clearAuth() {
			this.user = null;
			this.token = null;
			this.isLoggedIn = false;
		},

		async initFromConfig(): Promise<void> {
			await this.fetchServerConfig();
			const token = api.config?.cloudSyncToken as string | undefined;
			if (token) {
				this.token = token;
				this.isLoggedIn = true;
				// Validate in background; keep login state on network errors
				this.fetchMe();
			}
		},
	},
});
