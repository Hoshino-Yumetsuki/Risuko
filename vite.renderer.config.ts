import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

const dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(dirname, "src/renderer/pages/index");
const outDir = path.resolve(dirname, "dist/outputs");

export default defineConfig({
	root: rootDir,
	base: "",
	plugins: [vue(), tailwindcss()],
	resolve: {
		alias: {
			"@": path.resolve(dirname, "src/renderer"),
			"@shared": path.resolve(dirname, "src/shared"),
			"@static": path.resolve(dirname, "static"),
		},
		extensions: [".mjs", ".js", ".ts", ".jsx", ".tsx", ".json", ".vue"],
	},
	server: {
		host: "127.0.0.1",
		port: 9080,
		strictPort: true,
	},
	build: {
		outDir,
		emptyOutDir: true,
		sourcemap: false,
		chunkSizeWarningLimit: 2000,
		minify: "oxc",
		rollupOptions: {
			input: {
				index: path.resolve(rootDir, "index.html"),
				tray: path.resolve(rootDir, "tray.html"),
				"clip-prompt": path.resolve(rootDir, "clip-prompt.html"),
			},
		},
	},
	define: {
		"process.env.NODE_ENV": JSON.stringify(
			process.env.NODE_ENV || "development",
		),
		__VUE_OPTIONS_API__: true,
		__VUE_PROD_DEVTOOLS__: false,
		__VUE_PROD_HYDRATION_MISMATCH_DETAILS__: false,
	},
});
