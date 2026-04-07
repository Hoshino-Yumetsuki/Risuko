# Motrix

<p>
  <a href="https://motrix.app">
    <img src="./static/512x512.png" width="256" alt="Motrix App Icon" />
  </a>
</p>

## A full-featured download manager

[![GitHub release](https://img.shields.io/github/v/release/agalwood/Motrix.svg)](https://github.com/agalwood/Motrix/releases) ![Build/release](https://github.com/agalwood/Motrix/workflows/Build/release/badge.svg) ![Total Downloads](https://img.shields.io/github/downloads/agalwood/Motrix/total.svg) ![Support Platforms](https://camo.githubusercontent.com/a50c47295f350646d08f2e1ccd797ceca3840e52/68747470733a2f2f696d672e736869656c64732e696f2f62616467652f706c6174666f726d2d6d61634f5325323025374325323057696e646f77732532302537432532304c696e75782d6c69676874677265792e737667)

English | [简体中文](./README-CN.md)

Motrix is a full-featured download manager that supports downloading HTTP, FTP, BitTorrent, Magnet, etc.

Motrix has a clean and easy to use interface. I hope you will like it 👻.

✈️ [Official Website](https://motrix-next.vercel.app) | 📖 [Manual](https://github.com/agalwood/Motrix/wiki)

## 💽 Installation

Download from [GitHub Releases](https://github.com/YueMiyuki/Motrix/releases) and install it.

## ✨ Features

- 🕹 Simple and clear user interface
- 🦄 Supports BitTorrent & Magnet
- ☑️ BitTorrent selective download
- 📡 Update tracker list every day automatically
- 🔌 UPnP & NAT-PMP Port Mapping
- 🎛 Up to 10 concurrent download tasks
- 🚀 Supports 64 threads in a single task
- 🚥 Supports speed limit
- 🕶 Mock User-Agent
- 🔔 Download completed Notification
- 🤖 Resident system tray for quick operation
- 📟 Tray speed meter displays real-time speed (Mac only)
- 🌑 Dark mode
- 🗑 Delete related files when removing tasks (optional)
- 🌍 I18n, [View supported languages](#-internationalization).
- 🛠 More features in development

## 🖥 User Interface

![motrix-screenshot-task-en.png](./static/readme/UI.png)

## ⌨️ Development

### Clone Code

```bash
git clone https://github.com/YueMiyuki/Motrix-NEXT
```

### Install Dependencies

Requires Node.js >= 22 and Rust >= 1.77.

```bash
cd Motrix-NEXT
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
  - ~90% less bundle size (219.3 MB -> 21.1 MB)
  - ~70% less memory usage (taking nearest tenth, ~400MB -> ~120MB)
  - ~70% less peak CPU usage (~140% -> ~40%)

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
