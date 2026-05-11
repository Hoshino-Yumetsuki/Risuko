export interface RssFeed {
	id: string;
	url: string;
	title: string;
	site_link: string;
	description: string;
	update_interval_secs: number;
	last_fetched_at: number | null;
	created_at: number;
	is_active: boolean;
	error_count: number;
}

export interface ParsedMeta {
	series?: string | null;
	year?: number | null;
	season?: number | null;
	episode?: number | null;
	episode_end?: number | null;
	absolute_episode?: number | null;
	quality_tags: string[];
	group?: string | null;
	source?: string | null;
	codec?: string | null;
	container?: string | null;
	language?: string | null;
	seeders?: number | null;
}

export interface RssItem {
	id: string;
	feed_id: string;
	title: string;
	link: string;
	pub_date: number | null;
	description: string;
	enclosure_url: string | null;
	enclosure_type: string | null;
	enclosure_length: number | null;
	is_read: boolean;
	is_downloaded: boolean;
	download_path?: string;
	parsed_meta?: ParsedMeta | null;
	matched_rule_id?: string | null;
	/** Inline media URLs scraped from the entry body (img/video/audio/source)
	 * plus extra enclosure links beyond the primary `enclosure_url`. */
	media_urls?: string[];
}

export type PatternKind = "contains" | "glob" | "regex";

export interface Pattern {
	value: string;
	kind: PatternKind;
	case_sensitive: boolean;
}

export type EpisodeSelector =
	| { type: "all" }
	| { type: "from"; start: number }
	| { type: "range"; start: number; end: number }
	| { type: "specific"; values: number[] };

export type Weekday = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";

export interface Schedule {
	days: Weekday[];
	start_hour: number;
	end_hour: number;
	tz_offset_min: number;
}

export type RuleMode = "any-match" | "best-match";

export interface RuleStats {
	last_matched_at: number | null;
	match_count: number;
	download_count: number;
}

export interface RssRule {
	id: string;
	name: string;
	is_active: boolean;
	auto_download: boolean;
	feed_ids: string[];
	priority: number;
	mode: RuleMode;
	title_must: Pattern[];
	title_must_not: Pattern[];
	min_size_bytes: number | null;
	max_size_bytes: number | null;
	min_seeders: number | null;
	series_filter: string | null;
	seasons: number[] | null;
	episodes: EpisodeSelector | null;
	quality_preferences: string[];
	required_qualities: string[];
	forbidden_qualities: string[];
	upgrade_existing: boolean;
	download_dir: string | null;
	filename_template: string | null;
	schedule: Schedule | null;
	cooldown_secs: number;
	stats: RuleStats;
}

export interface DryRunMatch {
	item_id: string;
	feed_id: string;
	title: string;
	matched: boolean;
	score: number;
	reason: string;
}

export function emptyRule(name = ""): RssRule {
	return {
		id: "",
		name,
		is_active: true,
		auto_download: false,
		feed_ids: [],
		priority: 0,
		mode: "any-match",
		title_must: [],
		title_must_not: [],
		min_size_bytes: null,
		max_size_bytes: null,
		min_seeders: null,
		series_filter: null,
		seasons: null,
		episodes: null,
		quality_preferences: [],
		required_qualities: [],
		forbidden_qualities: [],
		upgrade_existing: false,
		download_dir: null,
		filename_template: null,
		schedule: null,
		cooldown_secs: 0,
		stats: { last_matched_at: null, match_count: 0, download_count: 0 },
	};
}
