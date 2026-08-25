export const buildMagnetLink = (task, withTracker = false) => {
	const { bittorrent, infoHash } = task;
	const { info } = bittorrent;

	const params = [`magnet:?xt=urn:btih:${infoHash}`];
	if (info?.name) {
		params.push(`dn=${encodeURI(info.name)}`);
	}

	if (withTracker) {
		(bittorrent.announceList || []).forEach((tier) => {
			const urls = Array.isArray(tier) ? tier : [tier];
			for (const tracker of urls) {
				if (tracker) {
					params.push(`tr=${encodeURIComponent(tracker)}`);
				}
			}
		});
	}

	return params.join("&");
};
