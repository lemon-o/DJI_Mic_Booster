# USB_Mic_Booster 技术架构与设计文档 (Architecture & Technical Specification)

## 1. 项目背景与设计理念

### 1.1 痛点背景
各类 USB 麦克风（如 USB 桌面电容麦、USB-C 无线领夹麦如 DJI Mic / DJI Mic 2 / DJI Mic Mini 等）在直接通过 USB 或音频线直连 Windows PC 时，由于 Windows 原生声卡驱动与硬件输出电平标准匹配问题，经常出现录制或通话音量严重偏小的情况。

以往业界的解决方案通常依赖：
1. **宿主软件 (DAW)**：如 Reaper、Cubase、FL Studio 等，动辄消耗几百兆内存且界面极其繁琐；
2. **虚拟调音台**：如 Voicemeeter，配置晦涩、驱动易冲突且容易出现音频抖动；
3. **基于 Electron / WebView2 / Qt 编写的包装工具**：启动慢，空载运行内存常高达 100MB~200MB，甚至占用独立显卡驱动资源。

### 1.2 设计目标
USB_Mic_Booster 的设计目标是打造一款**极端轻量、零外部运行库依赖、极低延迟、开机即用**的原生 Windows 音频增益与重定向工具：
- **纯原生无依赖**：基于 Rust + 纯 Win32 API / GDI 打造，零外部 GUI 运行时（无 Electron / Qt / WPF / WebView2）。
- **极小二进制体积**：最终发布单文件仅约 **360 KB**。
- **极致内存开销**：常驻运行内存稳定在 **~5 MB** 左右，CPU 占用趋近于 0%。
- **专业级音频处理**：支持 0.0dB ~ +30.0dB 线性放大与 `tanh` 渐进式软限幅防爆音算法，端到端吞吐延迟低至 **~25ms**。
- **系统级无缝集成**：支持开机自启、托盘后台常驻、静默启动、防多开单实例智能前台唤醒。

---

## 2. 系统总体架构与音频数据流

```
                    ┌────────────────────────────────────────────────────────┐
                    │                USB_Mic_Booster (进程)                  │
                    │                                                        │
┌──────────────┐    │  ┌────────────────┐    增益放大    ┌────────────────┐  │    ┌────────────────┐
│ 物理硬件麦克风 │ ──┼─>│ WASAPI 输入流  │ ────────────> │ 软限幅防爆音    │  ├───>│ 虚拟音频线      │
│ (USB / DJI / │    │  │ (cpal capture) │    (Gain)     │ (Soft Limiter) │  │    │ (CABLE Input)  │
│ 任何输入设备)  │    │  └────────────────┘               └──────┬─────────┘  │    └────────┬───────┘
└──────────────┘    │                                           │            │             │
                    │                                           ▼            │             ▼
                    │                                  ┌─────────────────┐   │    ┌────────────────┐
                    │                                  │ 无锁环形缓冲区   │   │    │ 腾讯会议 / OBS │
                    │                                  │ (Lock-free RB)  │   │    │ Discord / 微信 │
                    │                                  └────────┬────────┘   │    │ (CABLE Output) │
                    │                                           │            │    └────────────────┘
                    │                                           ▼            │
                    │                                  ┌─────────────────┐   │
                    │                                  │ WASAPI 输出流   │   │
                    │                                  │ (cpal render)   │   │
                    │                                  └─────────────────┘   │
                    └────────────────────────────────────────────────────────┘
```

### 数据流管道阶段：
1. **输入采集 (Capture)**：WASAPI 捕获回调实时获取物理麦克风的 PCM 数据帧（支持 F32 与 I16 格式自动归一化）。
2. **数字增益计算 (Gain Multiplication)**：将配置的 dB 值转换为线性倍率并逐样本相乘。
3. **电平统计 (Peak Level Metering)**：实时提取块级最大绝对值，供 UI 层 30FPS 动态电平表渲染。
4. **软限幅防爆音 (Soft Limiter)**：对接近满幅的音频信号使用双曲正切曲线平滑压制，防止硬削波产生数字爆音。
5. **跨线程解耦 (RingBuffer)**：将处理后的浮点样本推入低延迟无锁环形队列（容量 2880 采样点，约 30ms）。
6. **输出渲染 (Render)**：WASAPI 输出流以极低延迟从环形队列消费样本，写入虚拟声卡设备（`CABLE Input`）。

---

## 3. 核心音频处理引擎设计

### 3.1 线性倍率转换与原子同步
增益控制通过 `AudioController` 结构实现 UI 线程与高优先级音频处理线程的安全无锁通信：

$$\text{Linear Gain} = 10^{\frac{\text{Gain}_{\text{dB}}}{20}}$$

```rust
pub struct AudioController {
    gain_linear: Arc<AtomicU32>,  // 线性倍率浮点数的二进制位存储
    muted: Arc<AtomicBool>,        // 静音原子开关
    peak_level: Arc<AtomicU32>,   // 实时峰值电平（供UI读取）
    last_error: Arc<Mutex<Option<String>>>,
}
```
- **无锁实时读取**：音频回调中通过 `gain_linear.load(Ordering::Relaxed)` 读取，避免任何互斥锁引起的优先级反转或音频断流。
- **平滑增益调节**：支持从 0.0 dB 到 30.0 dB 的微调，滑动条步进为 0.1 dB。

### 3.2 智能软限幅器 (Soft Limiter) 数学模型
当人声经过大倍数数字放大（例如 +14dB ~ +22dB）后，大声叫喊或爆破音很容易超出数字音频满刻度（$[-1.0, 1.0]$）。如果直接进行硬削波（Hard Clipping），会产生大量奇次谐波的高频毛刺，听感极度刺耳。

本系统实现了基于双曲正切函数（$\tanh$）的软饱和限幅器：

$$f(x) = \begin{cases} 
x, & |x| \le 0.85 \\
\frac{\tanh(0.9 \cdot x)}{0.9}, & |x| > 0.85 
\end{cases}$$

- **线性保真区 ($|x| \le 0.85$)**：信号完全线性直通，保持 100% 原汁原味的麦克风频响与动态细节。
- **平滑饱和区 ($|x| > 0.85$)**：平滑压缩超限能量，波峰圆润过渡，彻底消除硬削波破音。

```rust
if left.abs() > 0.85 {
    left = (left * 0.9).tanh() / 0.9;
}
if right.abs() > 0.85 {
    right = (right * 0.9).tanh() / 0.9;
}
```

### 3.3 低延迟环形缓冲区与防欠载设计
- 使用 `ringbuf::HeapRb::<f32>` 构建单生产者单消费者（SPSC）无锁环形队列。
- 队列尺寸定为 **2880 个样本**（48kHz 立体声下约 30ms 缓冲量）。
- 写入溢出保护策略：当由于设备时钟微小漂移导致环形缓冲满时，丢弃超额数据，保障系统**永远不产生累积延迟**。

---

## 4. Win32 原生扁平化 UI 架构

为了在保证现代 UI 质感的同时将内存开销压制到极限（5MB 以内），本项目摒弃了一切现代重量级 GUI 框架，采用纯 Windows C 级 API + GDI 自绘技术。

### 4.1 现代色彩规范与视觉设计
| 元素 | 色值 (HEX) | 视觉用途 |
| :--- | :--- | :--- |
| **窗体底色** | `#F8F9FA` | 浅灰柔和背景，现代简洁 |
| **卡片底色** | `#FFFFFF` | 纯白内容卡片，圆角卡片分块 |
| **卡片边框** | `#E2E8F0` | 浅灰极细分割线 |
| **主强调色** | `#2563EB` | 品牌高亮蓝（滑块激活槽、复选框选中） |
| **文本主色** | `#1E293B` | 深灰主标题与选项文字 |
| **文本副色** | `#64748B` | 次级提示与下拉箭头 |
| **电平安全区**| `#10B981` | 绿色安全电平 (0% ~ 70%) |
| **电平预警区**| `#F59E0B` | 橙黄过渡电平 (70% ~ 90%) |
| **电平危险区**| `#EF4444` | 红色爆音电平 (90% ~ 100%) |

### 4.2 纯原生扁平自绘组件清单
1. **扁平化下拉框 (Flat ComboBox)**：
   - 彻底解决原生 Win32 ComboBox 带有灰色立体边框和不可改样式的传统弊端；
   - 采用自绘按钮触发 Win32 原生上下文菜单（`TrackPopupMenu`），支持设备高亮勾选状态。
2. **扁平化交互滑块 (Flat Slider - `USBMicFlatSliderClass`)**：
   - 独立注册原生窗口类，接管 `WM_PAINT`、`WM_LBUTTONDOWN`、`WM_MOUSEMOVE`；
   - 绘制浅灰底槽 (`#E2E8F0`) + 蓝色进度槽 (`#2563EB`) + 带有微妙阴影的纯白圆形滑块 Thumb，支持鼠标精准拖拽。
3. **扁平化复选框 (Flat Checkbox)**：
   - 接管 `WM_DRAWITEM`，绘制 16x16 浅灰边框圆角矩形，选中时填充 `#2563EB` 并平滑绘制纯白勾选符号 `✓`。
4. **电平表 (Flat VU Meter)**：
   - 采用自定义 `STATIC` 自绘控件，结合 GDI 双缓冲与峰值指数衰减平滑算法（`peak * 0.84 + cur * 0.16`）。

---

## 5. 进程管理、防多开与系统交互

### 5.1 单实例检测与跨进程智能唤醒
为了防止用户不小心多次运行造成声卡占用冲突，本程序内置了基于命名互斥体（Mutex）的单实例保护机制：

```rust
let mutex_name = wide_str("Local\\USBMicBooster_SingleInstance_Mutex");
let h_mutex = CreateMutexW(null_mut(), 1, mutex_name.as_ptr());
if h_mutex != 0 && GetLastError() == ERROR_ALREADY_EXISTS {
    let class_name = wide_str("USBMicBoosterWin32Class");
    let mut hwnd_existing = FindWindowW(class_name.as_ptr(), null_mut());
    if hwnd_existing == 0 {
        let legacy_class_name = wide_str("DJIMicBoosterWin32Class");
        hwnd_existing = FindWindowW(legacy_class_name.as_ptr(), null_mut());
    }
    if hwnd_existing != 0 {
        PostMessageW(hwnd_existing, WM_APP_WAKEUP, 0, 0);
        ShowWindow(hwnd_existing, SW_SHOW);
        ShowWindow(hwnd_existing, SW_RESTORE);
        SetForegroundWindow(hwnd_existing);
    }
    CloseHandle(h_mutex);
    return;
}
```

- **解决 Windows 跨进程前台锁痛点**：Windows 系统通常禁止外部进程直接夺取前台焦点。当第二实例检测到已有进程时，通过向旧实例投递自定义消息 `WM_APP_WAKEUP`，由原有主 UI 线程自身执行 `ShowWindow(hwnd, SW_RESTORE)` 和 `SetForegroundWindow`，确保无论原窗口处于隐藏、最小化还是托盘状态，都能 100% 唤醒至前台。

### 5.2 开机自启与静默启动深度联动
- **静默启动 UX 原则**：用户在桌面主动双击程序时，必定期望看见主窗口；而开机随 Windows 启动时，用户期望不打扰无感启动。
- **注册表 `--silent` 命令行分流**：
  - 勾选“开机自启”且“静默启动”时，写入注册表：`"...\USB_Mic_Booster.exe" --silent`
  - 勾选“开机自启”但不勾选“静默启动”时，写入注册表：`"...\USB_Mic_Booster.exe"`
  - `main()` 解析命令行参数：若包含 `--silent` 则以 `SW_HIDE` 启动到托盘；若无参数（用户手动双击）则以 `SW_SHOW` 启动弹出界面。

### 5.3 托盘图标保活与 Explorer 崩溃自愈
1. **异步定时检活机制**：针对部分 Windows 11 环境在 `WM_CREATE` 阶段任务栏 Shell 尚未响应导致 `Shell_NotifyIconW` 失败的极端情况，在 30FPS 定时器中增加保活机制，未成功挂载时自动重试，确保托盘图标永不丢失。
2. **`TaskbarCreated` 消息监听**：调用 `RegisterWindowMessageW("TaskbarCreated")`，当 Windows 资源管理器（`explorer.exe`）发生重启或崩溃重建时，程序自动重新向系统托盘注册图标。

### 5.4 极端内存开销压缩优化
在主窗口初始化完成后，调用 Windows 内存管理器接口：
```rust
SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
```
主动促使系统将未使用的冷分页移出工作集，使得软件内存占用常驻保持在 **~5 MB** 左右。

---

## 6. 构建与发布优化规范

在 `Cargo.toml` 中配置了极致的发布优化规则：
```toml
[package]
name = "usb_mic_booster"
version = "0.2.0"
edition = "2021"

[[bin]]
name = "USB_Mic_Booster"
path = "src/main.rs"

[profile.release]
opt-level = 3        # 最大性能优化
lto = true           # 链接时全局优化 (Link-Time Optimization)
codegen-units = 1    # 单代码生成单元，最大化内联
panic = "abort"      # 移除栈展开回溯代码
strip = true         # 剥离所有符号表与调试元数据
```

并通过 `build.rs` 调用 Windows SDK 的 `rc.exe` 自动编译 `app.rc` 资源清单文件，将应用程序图标（ID 1）直接静态嵌入 PE 可执行文件中，实现无外部依赖的单文件分发。

---

## 7. 设备智能热插拔与独立增益记忆

### 7.1 独立设备增益记忆 (Per-Device Gain Memory)
为了应对用户频繁插拔切换不同设备（例如桌面电容麦需要 +6dB，而无线领夹麦需要 +22dB）的痛点，配置引擎采用键值映射：
```rust
pub struct AppConfig {
    ...
    pub device_gains: HashMap<String, f32>,
    pub device_history: Vec<String>,
}
```
- **增益独立隔离**：每款麦克风单独记忆上次配置的 dB 增益值；
- **旧版平滑升级**：自动迁移旧版全局单值配置，向后兼容；
- **切换瞬时应用**：设备发生变动时，自动加载该设备的增益值、重置音频增益倍率并同步刷新 UI 滑块与预设按钮。

### 7.2 实时热插拔监听与智能回退栈 (Hotplug & Fallback Stack)
为实现“插上新麦克风自动切过去，拔下时自动平滑回退到上一个设备”的高可用体验：
1. **系统级消息监听**：捕获 Windows 顶层广播消息 `WM_DEVICECHANGE (0x0219)`，并设置 350ms 去抖定时器（等待 WASAPI 端点完全就绪）。
2. **音频流中断即时感知**：当正在工作的麦克风被物理拔除时，WASAPI 渲染流回调立即捕获异常并存入 `controller.last_error`，在 33ms 定时器内实现毫秒级即时断开感知。
3. **心跳双重保障**：每隔 1 秒执行静默设备扫描，防止蓝牙/无线驱动未广播标准硬件插拔消息。
4. **MRU 历史回退栈 (Most Recently Used Stack)**：
   - 插入新麦克风：将当前设备压入历史栈底，切换至新麦克风并记录到栈顶，加载新设备专属增益并无缝重启流；
   - 拔出当前麦克风：逆序遍历历史记录栈，选取最近使用过的仍在线设备（MRU Fallback），恢复其专属增益并无缝接续音频流；
   - 设备全部拔除：安全重置界面状态，显示“无可用麦克风”，停止音频管道避免空转。

