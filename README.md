# Risuko

<p>
  <a href="https://risuko.vercel.app">
    <img src="./static/512x512.png" width="256" alt="Risuko App Icon" />
  </a>
</p>

## A full-featured download manager

![GitHub_Release](https://img.shields.io/github/v/release/yuemiyuki/risuko?include_prereleases&style=for-the-badge&link=https%3A%2F%2Fgithub.com%2FYueMiyuki%2FRisuko%2Freleases)
![Rust](https://img.shields.io/badge/Rust-black?style=for-the-badge&logo=rust&logoColor=#E57324)
![Vue](https://img.shields.io/badge/Vue%20js-35495E?style=for-the-badge&logo=vuedotjs&logoColor=4FC08D)
![Vite](https://img.shields.io/badge/Vite-B73BFE?style=for-the-badge&logo=vite&logoColor=FFD62E)

English | [简体中文](./README-CN.md)

Risuko is a full-featured download manager that supports downloading HTTP, FTP, BitTorrent, Magnet, etc.

Risuko has a clean and easy to use interface. I hope you will like it 👻.

✈️ [Official Website](https://risuko.vercel.app)

## 💽 Installation

### Github release
Download from [GitHub Releases](https://github.com/YueMiyuki/Risuko/releases) and install it.

### NPM Package
Risuko provide a WebUI together with the engine
```
pnpm install -g @risuko/app
risuko-app --port 8080
```

Or Install CLI only
```
pnpm install -g @risuko/cli
risuko-cli --help
```


## 🖥 User Interface

![risuko-screenshot-task-en.png](./static/readme/UI.png)

## ⌨️ Development

### Clone Code

```bash
git clone https://github.com/YueMiyuki/Risuko
```

### Install Dependencies

Requires Node.js >= 22 and Rust >= 1.77.

```bash
cd risuko
pnpm install
```

### Dev Mode

```bash
pnpm run dev
```

### Build Release

```bash
pnpm run build
```

## 🛠 Technology Stack

- [Tauri v2](https://v2.tauri.app/)
- [Vue 3](https://vuejs.org/) + [Pinia](https://pinia.vuejs.org/) + [shadcn-vue](https://www.shadcn-vue.com/)
- [Vite](https://vite.dev/)
- [TypeScript](https://www.typescriptlang.org/)
- [Tailwind CSS](https://tailwindcss.com/)

## ☑️ TODO

See pinned issues for roadmap

## 🤝 Contribute [![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat)](http://makeapullrequest.com)

If you are interested in participating in joint development, PR and Forks are welcome!

## Optimizations

The Next version use half the memory comparing to original, also significantly less CPU usage bursts  
In Next v0.1.0, there is performance optimization because aria2 is replaced by native Rust  
All captured while idle, with command `psrecord <PID> --plot memory.png --include-children --duration 60`  
App info provided by Finder  
v0.1.0 has a singnificantly less CPU and memory usage, and smaller bundle size comparing to v0.4.0-alpha
Comparing to original, v0.1.0 has:
  - ~92% less bundle size (219.3 MB -> 17.3 MB)
  - ~70% less memory usage (taking nearest tenth, ~400MB -> ~120MB)
  - ~70% less peak CPU usage (~140% -> ~40%)

This is achieved by using rust build params:
```
[profile.release]
opt-level = 3
strip = "symbols"
lto = true
codegen-units = 1
panic = "abort"
```
It tells `rustc` to prioritize the binary over patience:

`opt-level = 3`  
Enables every LLVM optimization pass. builds get noticeably slower; program gets faster  
`strip = "symbols"`  
Strips debug symbols before shipping. The file shrinks, but if it crashes in production, we're staring at assembly  
`lto = true`  
Link Time Optimization across all crates. LLVM inlines across boundaries and deletes dead code
`codegen-units = 1`  
Forces the compiler to use a single translation unit. No parallel codegen, but LLVM sees the whole program for better optimization  
`panic = "abort"`  
Crash immediately on panic—no unwinding, no cleanup. Smaller binaries, but destructors don't run  

The small bundle size, cpu and memory performance is also acheived by removing aria2, and replace by native rust codes.


| Orignal | Next | Next v0.1.0 |
| ------- | ---- | ----------- |
| ![orignal_mem](./static/readme/Original_Memory.png) | ![0.4.0_mem](./static/readme/v0.0.4_Memory.png) | ![0.1.0_mem](./static/readme/v0.1.0_Memory.png) |
| ![original_appinfo](./static/readme/Original_AppInfo.png) | ![0.4.0_appinfo](./static/readme/v0.0.4_AppInfo.png) | ![0.1.0_appinfo](./static/readme/v0.1.0_AppInfo.png)

This is generated with ![psrecord](https://github.com/astrofrog/psrecord)

## 🌍 Internationalization

Translations into versions for other languages are welcome 🧐! Please read the [translation guide](./docs/CONTRIBUTING.md#-translation-guide) before starting translations.

| Key   | Name                | Status                                                                                                      |
| ----- | :------------------ | :---------------------------------------------------------------------------------------------------------- |
| ar    | Arabic              | ✔️ [@hadialqattan](https://github.com/hadialqattan), [@AhmedElTabarani](https://github.com/AhmedElTabarani) |
| bg    | Българският език    | ✔️ [@null-none](https://github.com/null-none)                                                               |
| ca    | Català              | ✔️ [@marcizhu](https://github.com/marcizhu)                                                                 |
| de    | Deutsch             | ✔️ [@Schloemicher](https://github.com/Schloemicher)                                                         |
| el    | Ελληνικά            | ✔️ [@Likecinema](https://github.com/Likecinema)                                                             |
| en-US | English             | ✔️                                                                                                          |
| es    | Español             | ✔️ [@Chofito](https://github.com/Chofito)                                                                   |
| fa    | فارسی               | ✔️ [@Nima-Ra](https://github.com/Nima-Ra)                                                                   |
| fr    | Français            | ✔️ [@gpatarin](https://github.com/gpatarin)                                                                 |
| hu    | Hungarian           | ✔️ [@zalnaRs](https://github.com/zalnaRs)                                                                   |
| id    | Indonesia           | ✔️ [@aarestu](https://github.com/aarestu)                                                                   |
| it    | Italiano            | ✔️ [@blackcat-917](https://github.com/blackcat-917)                                                         |
| ja    | 日本語              | ✔️ [@hbkrkzk](https://github.com/hbkrkzk)                                                                   |
| ko    | 한국어              | ✔️ [@KOZ39](https://github.com/KOZ39)                                                                       |
| nb    | Norsk Bokmål        | ✔️ [@rubjo](https://github.com/rubjo)                                                                       |
| nl    | Nederlands          | ✔️ [@nickbouwhuis](https://github.com/nickbouwhuis)                                                         |
| pl    | Polski              | ✔️ [@KanarekLife](https://github.com/KanarekLife)                                                           |
| pt-BR | Portuguese (Brazil) | ✔️ [@andrenoberto](https://github.com/andrenoberto)                                                         |
| ro    | Română              | ✔️ [@alyn3d](https://github.com/alyn3d)                                                                     |
| ru    | Русский             | ✔️ [@bladeaweb](https://github.com/bladeaweb)                                                               |
| th    | แบบไทย              | ✔️ [@nxanywhere](https://github.com/nxanywhere)                                                             |
| tr    | Türkçe              | ✔️ [@abdullah](https://github.com/abdullah)                                                                 |
| uk    | Українська          | ✔️ [@bladeaweb](https://github.com/bladeaweb)                                                               |
| vi    | Tiếng Việt          | ✔️ [@duythanhvn](https://github.com/duythanhvn)                                                             |
| zh-CN | 简体中文            | ✔️                                                                                                          |
| zh-TW | 繁體中文            | ✔️ [@Yukaii](https://github.com/Yukaii) [@5idereal](https://github.com/5idereal)                            |

## 📜 License

[MIT](https://opensource.org/licenses/MIT) Copyright (c) 2026-present YueMiyuki

Original project from [agalwood](https://github.com/agalwood/Motrix)  
Last update of the original project is already 3yrs ago, and I am a heavy user of Motrix  
Thanks to that bro for open sourcing this great project  
Wherever bro is, whatever bro is doing, I just hope bros doing well
:D
