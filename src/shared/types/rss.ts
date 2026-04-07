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
}

export interface RssRule {
	id: string;
	feed_id: string | null;
	name: string;
	pattern: string;
	is_regex: boolean;
	is_active: boolean;
	auto_download: boolean;
	download_dir: string | null;
}
