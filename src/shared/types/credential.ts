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
	vaulted?: boolean;
	clearVault?: boolean;
}

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
