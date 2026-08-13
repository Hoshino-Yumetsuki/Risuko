import { TRAY_CANVAS_CONFIG } from "@shared/constants";
import { draw } from "@shared/utils/tray";

let canvas: OffscreenCanvas | undefined;

interface TrayDrawPayload {
	theme: string;
	icon: ImageBitmap;
	uploadSpeed: string;
	downloadSpeed: string;
	showSpeed?: boolean;
	scale: number;
	resultType: string;
}

const drawTray = async (payload: TrayDrawPayload) => {
	canvas ??= new OffscreenCanvas(
		TRAY_CANVAS_CONFIG.WIDTH,
		TRAY_CANVAS_CONFIG.HEIGHT,
	);

	try {
		await draw({
			canvas,
			...payload,
		});

		const ctx = canvas.getContext("2d")!;
		const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);

		self.postMessage({
			type: "tray:drawed",
			payload: {
				rgba: Array.from(imageData.data),
				width: canvas.width,
				height: canvas.height,
			},
		});
	} catch (error: unknown) {
		logger(error instanceof Error ? error.message : String(error));
	}
};

const logger = (text: string) => {
	self.postMessage({
		type: "log",
		payload: text,
	});
};

self.postMessage({
	type: "initialized",
	payload: Date.now(),
});

self.addEventListener("message", (event) => {
	const { type, payload } = event.data;
	switch (type) {
		case "tray:draw":
			drawTray(payload);
			break;
		default:
			logger(JSON.stringify(event.data));
	}
});
