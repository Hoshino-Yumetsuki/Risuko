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
}
