// cd src-tauri && cargo build --release -p risuko-napi
// cp src-tauri/target/release/librisuko_napi.dylib packages/risuko-js/risuko.darwin-arm64.node
// codesign -s - packages/risuko-js/risuko.darwin-arm64.node

import risuko from "../packages/risuko-js/index.js";

(async () => {
	await risuko.startEngine();
	risuko.onEvent((event, gid) => {
		console.log(gid, event);
	});
	const gid = await risuko.addUri([
		"https://cdn.hotelnearmedanta.com/testfile.org/testfile.org-5GB.dat",
	]);
	console.log("Started download, GID:", gid);
})();
