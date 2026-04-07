/// <reference types="vite/client" />

declare module "*?worker" {
	const WorkerFactory: {
		new (): Worker;
	};
	export default WorkerFactory;
}

interface MotrixApp {
	commands: import("@/components/CommandManager").default;
	trayWorker: Worker;
	[key: string]: unknown;
}

interface Window {
	__app: MotrixApp;
}
