# USB_Mic_Booster (USB麦克风增益助手)

<p align="center">
  <img src="https://img.shields.io/badge/Platform-Windows-0078D6?logo=windows&logoColor=white" alt="Platform" />
  <img src="https://img.shields.io/badge/Language-Rust-dea584?logo=rust&logoColor=white" alt="Rust" />
  <img src="https://img.shields.io/badge/Latency-~25ms-brightgreen" alt="Low Latency" />
  <img src="https://img.shields.io/badge/Memory-~5MB-blue" alt="Memory Usage" />
  <img src="https://img.shields.io/badge/Size-~360KB-informational" alt="Binary Size" />
  <img src="https://img.shields.io/badge/License-MIT-green" alt="License" />
</p>

**USB_Mic_Booster (USB麦克风增益助手)** 是一款专为解决各类 **USB 麦克风**（包含 USB 桌面电容麦、USB 无线领夹麦如 DJI Mic / DJI Mic 2 / DJI Mic Mini、各类板载/外置声卡麦克风）在 Windows 系统下直连收音音量偏小问题而设计的高性能、超轻量级实时数字增益与音频重定向工具。

无需复杂的宿主软件（DAW）或笨重的虚拟调音台，USB_Mic_Booster 通过 Windows 底层 WASAPI 原生音频流捕获与处理，提供最高 **+30 dB** 的纯净数字增益及智能防爆音软限幅，并无缝路由至虚拟声卡供全系统软件使用。

---

## 📚 项目文档导航

- 📖 **[用户使用与场景配置手册 (User Guide)](docs/USER_GUIDE.md)**：包含驱动安装、软件配置、腾讯会议/OBS/Discord/微信实战接入教程与常见问题排查（FAQ）。
- 🏗️ **[技术架构与设计文档 (Architecture Specification)](docs/ARCHITECTURE.md)**：深入剖析 WASAPI 音频管道、双曲正切软限幅算法、无锁环形队列、Win32 扁平自绘机制与 5MB 极端内存优化原理。

---

## ✨ 核心特性

- 🚀 **极致轻量，单文件绿色免安装**
  - 基于 Rust 纯原生 Win32 API / GDI 打造，**零外部重型 GUI 依赖**（无 Electron、无 WebView2、无 Qt）。
  - 单可执行文件仅约 **360 KB**，常驻运行内存稳定在 **~5 MB**，CPU 占用趋近于 0%，零 GPU/显卡驱动开销。
- 🎨 **现代扁平化浅色视觉设计**
  - 纯净白色卡片搭配清爽淡灰背景（`#F8F9FA`），彻底摆脱传统 Win32 的老旧灰框。
  - 全套原生扁平化自绘控件：自绘扁平下拉选单（Flat ComboBox）、扁平滑块（Flat Slider）、扁平复选框（Flat Checkbox）与低延迟电平表。
- 🔊 **实时透明数字增益**
  - 支持 `0.0 dB` 到 `+30.0 dB` 线性增益微调，步进精度达 0.1 dB。
  - 内置精选常用快捷预设按钮：`+6dB`、`+12dB`、`+14dB`、`+18dB`、`+22dB`，一键满足不同距离与人声响度需求。
- 🛡️ **智能软限幅防爆音 (Soft Limiter)**
  - 内置基于双曲正切曲线（`tanh`）的平滑软限幅饱和算法。
  - 在大幅放大人声细节的同时，平滑压制大声喊叫或爆破音，彻底消除数字硬削波破音。
- ⚡ **超低延迟架构**
  - 底层基于 `cpal` 原生音频接口结合无锁环形缓冲区（Lock-free RingBuffer）。
  - 音频吞吐端到端延迟低至 **~25ms - 30ms**，直播对口型、游戏开黑连麦、会议沟通完全无感。
- 📊 **动态电平峰值指示 (Peak Meter)**
  - 实时可视化音频电平柱状图，直观掌握输入/输出音量与爆音风险。
- 📌 **系统托盘与开机自启**
  - 支持关闭窗口自动最小化至系统托盘常驻运行，单击或双击托盘图标即可唤醒主界面。
  - 托盘右键菜单支持：打开主界面、一键静音、快速开关开机自启与静默启动、彻底退出程序。
  - **智能静默启动**：与 Windows 开机自启联动，开机时直接后台驻留托盘免打扰；手动双击打开时必定弹出主界面，符合直觉。
- 🔒 **单实例保护与智能唤醒**
  - 内置进程互斥锁，彻底防止重复运行造成声卡冲突。
  - 重复双击时自动通过内部消息唤醒并前台置顶已有主窗口。
  - 监听 `TaskbarCreated` 系统消息，资源管理器（Explorer）重启或崩溃时自动重绘托盘图标。

---

## 🎧 工作原理

```
┌─────────────────────────┐       ┌────────────────────────┐       ┌────────────────────────┐       ┌──────────────────────┐
│  物理硬件麦克风输入     │ ───>  │    USB_Mic_Booster     │ ───>  │   CABLE Input (虚拟线) │ ───>  │ 腾讯会议 / OBS /      │
│  (USB / DJI / 声卡输入) │       │ 数字增益 + 软限幅防爆音 │       │ VB-Audio Virtual Cable │       │ Discord / 微信等通话 │
└─────────────────────────┘       └────────────────────────┘       └────────────────────────┘       └──────────────────────┘
```

---

## 🚀 快速上手

### 步骤 1：安装虚拟声卡驱动（首次需安装）
为了让其他软件（如腾讯会议、微信、Discord、OBS 等）读取增益后的声音，需要使用虚拟音频线：
1. 打开本项目内置的 `VBCABLE_Driver` 文件夹（或前往 [VB-Audio 官网](https://vb-audio.com/Cable/) 下载）。
2. 右键单击 `VBCABLE_Setup_x64.exe`，选择 **以管理员身份运行** 并完成安装。
3. 安装完成后建议根据提示重启电脑以激活虚拟设备。

### 步骤 2：运行 USB_Mic_Booster
1. 下载 Release 页面中的 `USB_Mic_Booster.exe`（单文件，双击即可运行）。
2. 在主界面中进行配置：
   - **输入设备**：选择您的硬件麦克风（如 USB 麦克风、`DJI Mic` 或对应声卡麦克风输入）。
   - **输出设备**：程序后台全自动寻找并绑定 `CABLE Input (VB-Audio Virtual Cable)`。
   - **音量增益**：拖动滑块或点击预设按钮调整增益（推荐从 `+12dB` 或 `+14dB` 开始试听）。
3. 勾选 **开机自启动** 与 **静默启动**（可选），方便平时开机免打扰无感运行。
4. 点击关闭或最小化按钮，程序将自动驻留到右下角系统托盘。

### 步骤 3：在通讯/录音软件中选用虚拟麦克风
打开你要使用的软件（如 Discord、腾讯会议、Zoom、OBS Studio、微信等）：
- 将软件的 **麦克风 / 音频输入设备** 设置为：
  👉 `CABLE Output (VB-Audio Virtual Cable)`
- 此时对方即可听到经由 USB_Mic_Booster 放大饱满且防爆音的高品质人声音频！

---

## 🛠️ 本地编译与构建

本项目使用 Rust 编写，需配置 Rust 开发环境（推荐 Rust 1.70+ 及 MSVC 工具链）。

### 1. 克隆仓库
```bash
git clone https://github.com/lemon-o/USB_Mic_Booster.git
cd USB_Mic_Booster
```

### 2. 调试运行
```bash
cargo run
```

### 3. 发布打包
```bash
cargo build --release
```
编译产物位于 `target/release/USB_Mic_Booster.exe`。
已在 `.cargo/config.toml` 中配置 Common-Controls 6.0 清单支持，并在 `Cargo.toml` 中启用了 LTO 与尺寸裁剪优化，生成的二进制文件小巧高效（~360 KB）。

---

## ⚙️ 配置文件说明

程序首次运行后，会在同级目录下生成 `booster_config.json` 记录用户偏好：

```json
{
  "gain_db": 14.0,
  "input_device": "USB Microphone",
  "output_device": "CABLE Input (VB-Audio Virtual Cable)",
  "silent_start": false,
  "auto_start": false
}
```

- `gain_db`：上次保存的增益分贝数值（默认 14.0 dB）。
- `input_device`：上次选择的输入音频设备名。
- `output_device`：上次选择的输出音频设备名。
- `silent_start`：是否静默启动（默认 `false`）。
- `auto_start`：是否开机自启（默认 `false`）。

---

## ❓ 常见问题摘要

> 更多问题排查请查阅详细文档：[docs/USER_GUIDE.md](docs/USER_GUIDE.md)

<details>
<summary><b>Q1: 为什么有轻微的声音延迟？</b></summary>
USB_Mic_Booster 经过高度优化，音频缓冲区环形队列仅设为约 25ms~30ms，通常人耳在 40ms 以内无法感知延迟。如果感受到明显延迟，请检查是否在系统声音面板中开启了“侦听此设备”，或者系统声卡驱动采样率是否统一（推荐将输入和输出设备均设为 48000Hz 16/24位）。
</details>

<details>
<summary><b>Q2: 声音变大后会不会爆音或失真？</b></summary>
程序内部集成了平滑软限幅器（Soft Limiter）。当信号幅度接近满幅（0.85 以上）时，算法自动进入双曲正切渐进压缩曲线，有效消除硬削波破音。但建议增益适度，以电平指示条峰值处于安全绿色/黄色区间为佳。
</details>

<details>
<summary><b>Q3: 关闭窗口后程序还在运行吗？</b></summary>
是的，点击右上角关闭按钮将最小化隐藏到系统右下角托盘图标。左键单击或双击托盘图标即可打开控制面板，右键可切换静音或彻底退出。
</details>

---

## 📄 License

本项目采用 [MIT License](LICENSE) 开源协议。
