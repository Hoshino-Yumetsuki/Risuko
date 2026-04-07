import { spawn } from "node:child_process";
import cfonts from "cfonts";
import chalk from "chalk";

const isCI = process.env.CI || false;

function greeting() {
	const cols = process.stdout.columns;
	let text = "";

	if (cols > 104) {
		text = "motrix-dev";
	} else if (cols > 76) {
		text = "motrix-|dev";
	} else {
		text = false;
	}

	if (text && !isCI) {
		cfonts.say(text, {
			colors: ["magentaBright"],
			font: "simple3d",
			space: false,
		});
	} else {
		console.log(chalk.magentaBright.bold("\n  motrix-dev"));
	}

	console.log(`${chalk.blue("  getting ready...")}\n`);
}

greeting();

const child = spawn("pnpm", ["tauri", "dev"], {
	stdio: "inherit",
	shell: true,
	env: { ...process.env },
});

child.on("close", (code) => {
	process.exit(code);
});
