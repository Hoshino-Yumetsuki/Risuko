export type LogLevel =
	| "trace"
	| "debug"
	| "info"
	| "warn"
	| "error"
	| "unknown";

export interface LogFileSummary {
	name: string;
	date?: string | null;
	sizeBytes: number;
	modifiedAtMs: number;
}

export interface LogEntry {
	lineNumber: number;
	timestamp?: string | null;
	level: LogLevel;
	message: string;
	raw: string;
}

export interface LogReadResult {
	name: string;
	entries: LogEntry[];
	truncated: boolean;
	bytesRead: number;
	totalBytes: number;
	totalLines: number;
	returnedLines: number;
}
