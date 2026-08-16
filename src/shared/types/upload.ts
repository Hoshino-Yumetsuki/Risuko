export type UploadProtocol = "webdav" | "s3" | "sftp" | "ftp";

interface WebdavConfig {
	endpoint: string;
	basePath: string;
	username?: string;
	password?: string;
	insecure?: boolean;
}

interface S3Config {
	endpoint: string;
	region: string;
	bucket: string;
	accessKeyId: string;
	secretAccessKey?: string;
	prefix?: string;
	forcePathStyle?: boolean;
}

interface SftpConfig {
	host: string;
	port: number;
	username: string;
	password?: string;
	privateKey?: string;
	basePath?: string;
}

interface FtpConfig {
	host: string;
	port: number;
	username?: string;
	password?: string;
	basePath?: string;
	secure?: boolean;
	insecure?: boolean;
}

export type SinkConfig =
	| ({ kind: "webdav" } & WebdavConfig)
	| ({ kind: "s3" } & S3Config)
	| ({ kind: "sftp" } & SftpConfig)
	| ({ kind: "ftp" } & FtpConfig);

type PostUploadAction = "keep" | "trash" | "move";

export interface UploadSinkRecord {
	id: string;
	label: string;
	config: SinkConfig;
	postAction: PostUploadAction;
	moveTarget?: string | null;
	createdAt: number;
	lastUsedAt?: number | null;
}

interface RuleMatch {
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
