import { getApiErrorMessage } from "@shared/utils";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import axios from "axios";
import { defineStore } from "pinia";
import { useAuthStore } from "./auth";

const shareAxios = axios.create({ timeout: 15_000 });

export type ShareDirection = "send" | "receive";

type ShareStatus =
	| "preparing"
	| "waiting"
	| "connecting"
	| "transferring"
	| "completed"
	| "terminated"
	| "error"
	| "cancelled";

export type SharePath = "direct" | "relay" | "mixed" | "unknown";

export interface ShareFile {
	name: string;
	size: number;
}

export interface ShareTransfer {
	id: string;
	direction: ShareDirection;
	status: ShareStatus;
	files: ShareFile[];
	deviceCode?: string;
	url?: string;
	shareId?: string;
	path: SharePath;
	transferred: number;
	total: number;
	speedBps: number;
	error?: string;
	destDir?: string;
	createdAt: number;
}

interface ShareEventPayload {
	id: string;
	kind:
		| "connected"
		| "path"
		| "progress"
		| "completed"
		| "terminated"
		| "error";
	path?: SharePath;
	transferred?: number;
	total?: number;
	message?: string;
}

interface ResolvedSession {
	shareId: string;
	deviceCode: string;
	ticket: string | null;
	files: ShareFile[];
	expiresAt: number;
}

interface ShareState {
	role: ShareDirection;
	transfers: ShareTransfer[];
	pendingIncoming: ResolvedSession | null;
	pendingShareId: string | null;
	resolving: boolean;
	errorMessage: string;
}

let listenerRegistered = false;
const speedSamples = new Map<
	string,
	{ at: number; bytes: number; speed: number }
>();

function updateTransferSpeed(
	transfer: ShareTransfer,
	transferred: number,
): void {
	const now = Date.now();
	const prev = speedSamples.get(transfer.id);
	let speed = transfer.speedBps ?? 0;
	if (prev && now > prev.at) {
		const dt = (now - prev.at) / 1000;
		const delta = transferred - prev.bytes;
		if (dt >= 0.25 && delta >= 0) {
			const instant = delta / dt;
			speed = speed > 0 ? speed * 0.6 + instant * 0.4 : instant;
		} else {
			speed = prev.speed;
		}
	}
	speedSamples.set(transfer.id, { at: now, bytes: transferred, speed });
	transfer.speedBps = speed;
}

function clearTransferSpeed(id: string): void {
	speedSamples.delete(id);
}

export function parseShareInput(raw: string): string {
	const trimmed = raw.trim();
	if (!trimmed) {
		return "";
	}
	try {
		const url = new URL(trimmed);
		const segment = url.pathname.split("/").filter(Boolean).pop();
		if (segment) {
			return decodeURIComponent(segment).toUpperCase();
		}
	} catch {}
	const deep = trimmed.match(/risuko:\/\/share\/([^/?#]+)/i);
	if (deep?.[1]) {
		return decodeURIComponent(deep[1]).toUpperCase();
	}
	return trimmed.toUpperCase();
}

function newId(): string {
	try {
		return crypto.randomUUID();
	} catch {
		return `share-${Date.now()}-${Math.random().toString(36).slice(2)}`;
	}
}

function isTerminalShareResolveError(err: unknown): boolean {
	if (!axios.isAxiosError(err) || !err.response?.status) {
		return false;
	}
	const status = err.response.status;
	return status >= 400 && status < 500 && status !== 429;
}

export const useShareStore = defineStore("share", {
	state: (): ShareState => ({
		role: "send",
		transfers: [],
		pendingIncoming: null,
		pendingShareId: null,
		resolving: false,
		errorMessage: "",
	}),
	getters: {
		serverUrl(): string {
			return useAuthStore().serverUrl;
		},
	},
	actions: {
		authHeaders(): Record<string, string> {
			return useAuthStore().getAuthHeaders();
		},

		async ensureListener(): Promise<void> {
			if (listenerRegistered) {
				return;
			}
			listenerRegistered = true;
			try {
				const { listen } = await import("@tauri-apps/api/event");
				await listen<ShareEventPayload>("share:event", (event) => {
					this.applyEvent(event.payload);
				});
			} catch (err) {
				listenerRegistered = false;
				logger.warn("[Risuko] failed to register share listener:", err);
			}
		},

		applyEvent(payload: ShareEventPayload): void {
			const transfer = this.transfers.find((t) => t.id === payload.id);
			if (!transfer) {
				return;
			}
			switch (payload.kind) {
				case "connected":
					if (transfer.status !== "transferring") {
						transfer.status = "transferring";
					}
					break;
				case "path":
					if (payload.path) {
						transfer.path = payload.path;
					}
					break;
				case "progress":
					transfer.status = "transferring";
					transfer.transferred = payload.transferred ?? transfer.transferred;
					if (payload.total && payload.total > 0) {
						transfer.total = payload.total;
					}
					updateTransferSpeed(transfer, transfer.transferred);
					break;
				case "completed":
					if (
						transfer.total > 0 &&
						transfer.transferred < transfer.total &&
						transfer.status !== "cancelled"
					) {
						transfer.status = "terminated";
					} else {
						transfer.status = "completed";
						if (transfer.total > 0) {
							transfer.transferred = transfer.total;
						}
					}
					clearTransferSpeed(transfer.id);
					break;
				case "terminated":
					if (transfer.status !== "cancelled") {
						transfer.status = "terminated";
					}
					clearTransferSpeed(transfer.id);
					break;
				case "error":
					if (
						transfer.status !== "cancelled" &&
						transfer.total > 0 &&
						transfer.transferred > 0 &&
						transfer.transferred < transfer.total
					) {
						transfer.status = "terminated";
					} else if (transfer.status !== "cancelled") {
						transfer.status = "error";
					}
					transfer.error = payload.message || "Transfer failed";
					clearTransferSpeed(transfer.id);
					break;
			}
		},

		patchTransfer(id: string, partial: Partial<ShareTransfer>): void {
			const transfer = this.transfers.find((t) => t.id === id);
			if (transfer) {
				Object.assign(transfer, partial);
			}
		},

		async startSend(paths: string[]): Promise<string> {
			if (!useAuthStore().isLoggedIn) {
				throw new Error("Login required");
			}
			await this.ensureListener();
			const id = newId();
			this.transfers.unshift({
				id,
				direction: "send",
				status: "preparing",
				files: [],
				path: "unknown",
				transferred: 0,
				total: 0,
				speedBps: 0,
				createdAt: Date.now(),
			});

			try {
				const info = await invoke<{ ticket: string; files: ShareFile[] }>(
					"share_start_send",
					{ id, paths },
				);
				this.patchTransfer(id, {
					files: info.files,
					total: info.files.reduce((sum, f) => sum + (f.size || 0), 0),
				});

				const { data } = await shareAxios.post(
					`${this.serverUrl}/share`,
					{ direction: "send", ticket: info.ticket, files: info.files },
					{ headers: this.authHeaders() },
				);
				this.patchTransfer(id, {
					shareId: data.shareId,
					deviceCode: data.deviceCode,
					url: data.url,
					status: "waiting",
				});
			} catch (err) {
				const message = getApiErrorMessage(err, "Share request failed");
				this.patchTransfer(id, { status: "error", error: message });
				await invoke("share_cancel", { id }).catch(() => undefined);
				throw new Error(message);
			}
			return id;
		},

		async resolveByCode(code: string): Promise<ResolvedSession> {
			const normalized = code.trim().toUpperCase();
			const { data } = await shareAxios.get(
				`${this.serverUrl}/share/code/${encodeURIComponent(normalized)}`,
				{ headers: this.authHeaders() },
			);
			return data as ResolvedSession;
		},

		async resolveById(shareId: string): Promise<ResolvedSession> {
			const { data } = await shareAxios.get(
				`${this.serverUrl}/share/${encodeURIComponent(shareId)}`,
				{ params: { format: "json" }, headers: this.authHeaders() },
			);
			return data as ResolvedSession;
		},

		async openFromShareId(shareId: string): Promise<void> {
			this.role = "receive";
			this.pendingShareId = shareId;
			if (!useAuthStore().isLoggedIn) {
				return;
			}
			this.resolving = true;
			this.errorMessage = "";
			try {
				await this.ensureListener();
				this.pendingIncoming = await this.resolveById(shareId);
				this.pendingShareId = null;
			} catch (err) {
				this.errorMessage = getApiErrorMessage(err, "Share request failed");
			} finally {
				this.resolving = false;
			}
		},

		async resumeAfterLogin(): Promise<void> {
			if (this.pendingShareId) {
				await this.openFromShareId(this.pendingShareId);
			}
		},

		async lookupCode(code: string): Promise<void> {
			this.resolving = true;
			this.errorMessage = "";
			try {
				await this.ensureListener();
				this.pendingIncoming = await this.resolveByCode(parseShareInput(code));
			} catch (err) {
				this.errorMessage = getApiErrorMessage(err, "Share request failed");
				this.pendingIncoming = null;
			} finally {
				this.resolving = false;
			}
		},

		async receivePending(destDir: string): Promise<string> {
			if (!useAuthStore().isLoggedIn) {
				throw new Error("Login required");
			}
			const session = this.pendingIncoming;
			if (!session) {
				throw new Error("No incoming share to receive");
			}
			await this.ensureListener();

			let ticket = session.ticket;
			let files = session.files;

			if (!ticket) {
				const resolved = await this.waitForTicket(session.shareId);
				ticket = resolved.ticket;
				files = resolved.files;
			}
			if (
				!this.pendingIncoming ||
				this.pendingIncoming.shareId !== session.shareId
			) {
				throw new Error("Receive cancelled");
			}
			if (!ticket) {
				throw new Error("Sender has not shared any files yet");
			}

			const id = newId();
			this.transfers.unshift({
				id,
				direction: "receive",
				status: "connecting",
				files,
				shareId: session.shareId,
				deviceCode: session.deviceCode,
				path: "unknown",
				transferred: 0,
				total: files.reduce((sum, f) => sum + (f.size || 0), 0),
				speedBps: 0,
				destDir,
				createdAt: Date.now(),
			});
			this.pendingIncoming = null;

			try {
				await invoke("share_start_receive", {
					id,
					ticket,
					destDir,
					files,
				});
			} catch (err) {
				const message = getApiErrorMessage(err, "Share request failed");
				this.patchTransfer(id, { status: "error", error: message });
				throw new Error(message);
			}
			return id;
		},

		async waitForTicket(
			shareId: string,
			attempts = 60,
		): Promise<ResolvedSession> {
			for (let i = 0; i < attempts; i += 1) {
				if (this.pendingIncoming?.shareId !== shareId) {
					throw new Error("Receive cancelled");
				}
				try {
					const session = await this.resolveById(shareId);
					if (session.ticket) {
						return session;
					}
				} catch (err) {
					if (isTerminalShareResolveError(err)) {
						throw err;
					}
					logger.warn("[Risuko] share ticket poll failed, retrying:", err);
				}
				await new Promise((resolve) => setTimeout(resolve, 2000));
			}
			throw new Error("Timed out waiting for the sender");
		},

		async cancel(id: string): Promise<void> {
			const transfer = this.transfers.find((t) => t.id === id);
			await invoke("share_cancel", { id }).catch(() => undefined);
			if (transfer) {
				if (transfer.shareId) {
					shareAxios
						.delete(`${this.serverUrl}/share/${transfer.shareId}`, {
							headers: this.authHeaders(),
						})
						.catch(() => undefined);
				}
				if (!["completed", "terminated", "error"].includes(transfer.status)) {
					transfer.status = "cancelled";
				}
				clearTransferSpeed(id);
			}
		},

		remove(id: string): void {
			clearTransferSpeed(id);
			const idx = this.transfers.findIndex((t) => t.id === id);
			if (idx >= 0) {
				this.transfers.splice(idx, 1);
			}
		},

		clearPending(): void {
			this.pendingIncoming = null;
			this.errorMessage = "";
		},
	},
});
