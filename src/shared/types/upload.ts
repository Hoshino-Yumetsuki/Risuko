// Cloud upload sink types
// Ref: `risuko-engine/src/engine/upload/{sink,rules,manager}.rs`

export type UploadProtocol = "webdav" | "s3" | "sftp" | "ftp";

export interface WebdavConfig {
	endpoint: string;
	basePath: string;
	username?: string | null;
	password?: string | null;
	insecure?: boolean;
}

export interface S3Config {
	endpoint: string;
	region: string;
	bucket: string;
	accessKeyId: string;
	secretAccessKey: string;
	prefix?: string;
	forcePathStyle?: boolean;
}

export interface SftpConfig {
	host: string;
	port: number;
	username: string;
	password?: string | null;
	privateKey?: string | null;
	basePath?: string;
}

export interface FtpConfig {
	host: string;
	port: number;
	username?: string | null;
	password?: string | null;
	basePath?: string;
	secure?: boolean;
}

export type SinkConfig =
	| ({ kind: "webdav" } & WebdavConfig)
	| ({ kind: "s3" } & S3Config)
	| ({ kind: "sftp" } & SftpConfig)
	| ({ kind: "ftp" } & FtpConfig);

export type PostUploadAction = "keep" | "trash" | "move";

export interface UploadSinkRecord {
	id: string;
	label: string;
	config: SinkConfig;
	postAction: PostUploadAction;
	moveTarget?: string | null;
	createdAt: number;
	lastUsedAt?: number | null;
}

export interface RuleMatch {
	categories: string[];
	extensions: string[];
	minSize?: number | null;
	maxSize?: number | null;
	taskKinds: string[];
}

export interface UploadRule {
	id: string;
	label: string;
	sinkId: string;
	match: RuleMatch;
	enabled: boolean;
}

export type UploadJobStatus =
	| "queued"
	| "active"
	| "complete"
	| "failed"
	| "cancelled";

export interface UploadJob {
	id: string;
	gid: string;
	sinkId: string;
	localPath: string;
	remoteRelative: string;
	size: number;
	uploaded: number;
	status: UploadJobStatus;
	error?: string | null;
	createdAt: number;
	startedAt?: number | null;
	finishedAt?: number | null;
}
