/**
 * A user-managed authentication profile applied when adding a download.
 *
 * Secret fields (the `CREDENTIAL_SECRET_FIELDS` set below) may be persisted
 * either inline in `user.json` (legacy, when `vaulted` is falsy) or in the
 * OS keychain via the Tauri `vault_*` commands (when `vaulted === true`).
 * The non-secret metadata always stays inline so the renderer can list and
 * match credentials without unlocking the keychain.
 */
export interface SavedCredential {
	id: string;
	label?: string;
	host?: string;
	protocol?: string;
	authorization?: string;
	cookie?: string;
	ftpUser?: string;
	ftpPasswd?: string;
	sftpPrivateKey?: string;
	sftpPrivateKeyContent?: string;
	sftpKeyPassphrase?: string;
	allProxy?: string;
	createdAt: number;
	lastUsedAt: number;
	/** True when the secret fields live in the OS keychain instead of inline. */
	vaulted?: boolean;
}

/** Sensitive fields that may be moved to the OS keychain. */
export const CREDENTIAL_SECRET_FIELDS = [
	"authorization",
	"cookie",
	"ftpUser",
	"ftpPasswd",
	"sftpPrivateKey",
	"sftpPrivateKeyContent",
	"sftpKeyPassphrase",
	"allProxy",
] as const satisfies ReadonlyArray<keyof SavedCredential>;
