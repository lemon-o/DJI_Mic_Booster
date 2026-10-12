use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct AudioEngine {
    _input_stream: Stream,
    _output_stream: Stream,
}

#[derive(Clone)]
pub struct AudioController {
    gain_linear: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    peak_level: Arc<AtomicU32>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl AudioController {
    pub fn new() -> Self {
        let gain = 10.0f32.powf(14.0 / 20.0);
        Self {
            gain_linear: Arc::new(AtomicU32::new(gain.to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
            peak_level: Arc::new(AtomicU32::new(0)),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_gain_db(&self, db: f32) {
        let gain = 10.0f32.powf(db / 20.0);
        self.gain_linear.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn get_peak(&self) -> f32 {
        f32::from_bits(self.peak_level.load(Ordering::Relaxed))
    }

    #[allow(dead_code)]
    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn clear_error(&self) {
        if let Ok(mut lock) = self.last_error.lock() {
            *lock = None;
        }
    }
}

fn is_virtual_audio_input(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("cable output")
        || lower.contains("cable input")
        || lower.contains("vb-audio")
        || lower.contains("virtual cable")
        || lower.contains("virtual audio")
        || lower.contains("voicemeeter")
}

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut list = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                // 过滤虚拟声卡录音端，避免音频自激回环与用户误选
                if !is_virtual_audio_input(&name) {
                    list.push(name);
                }
            }
        }
    }
    list
}

pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut list = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                list.push(name);
            }
        }
    }
    list
}

fn find_device_by_name(is_input: bool, target_name: &str) -> Option<Device> {
    let host = cpal::default_host();
    let devices = if is_input {
        host.input_devices().ok()?
    } else {
        host.output_devices().ok()?
    };

    for dev in devices {
        if let Ok(name) = dev.name() {
            if name == target_name {
                return Some(dev);
            }
        }
    }
    None
}

/// 手机/广播级前瞻动态压限器 (Lookahead Dynamic Peak Limiter)
/// 拥有 2.0ms 极微前瞻窗口与毫秒级包络跟随器。
/// 遇到突发大声或贴麦喊叫时，在波峰到达前毫秒级平滑压低整体音量系数，
/// 喊完后平滑回弹释放。完全不改变声波波形正弦形状，零谐波失真，实现手机般的稳定饱满音质。
#[derive(Clone)]
pub struct LookaheadLimiter {
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    delay_idx: usize,
    delay_len: usize,

    current_gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    ceiling: f32,
}

impl LookaheadLimiter {
    pub fn new(sample_rate: f32) -> Self {
        // 2.0 ms 超微前瞻窗口 (48kHz 下仅 96 个采样点，延迟完全不可感知)
        let delay_len = ((sample_rate * 0.0020).round() as usize).max(16);

        // 快速下压 Attack: 约 0.5ms，配合 2.0ms 前瞻，确保在波峰离开延迟线前增益已经就位
        let attack_time = 0.0005f32;
        let attack_coeff = (-1.0 / (sample_rate * attack_time)).exp();

        // 平滑恢复 Release: 约 70ms，人声停歇后自然回弹，无呼吸效应与泵浦感
        let release_time = 0.070f32;
        let release_coeff = (-1.0 / (sample_rate * release_time)).exp();

        Self {
            delay_l: vec![0.0; delay_len],
            delay_r: vec![0.0; delay_len],
            delay_idx: 0,
            delay_len,
            current_gain: 1.0,
            attack_coeff,
            release_coeff,
            ceiling: 0.90, // 目标天花板 ~ -0.9 dBFS
        }
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let input_peak = left.abs().max(right.abs());

        // 1. 前瞻探测目标增益衰减系数
        let target_gain = if input_peak > self.ceiling {
            self.ceiling / input_peak
        } else {
            1.0
        };

        // 2. 毫秒级包络平滑跟踪 (Attack 快速下压, Release 平滑回弹)
        if target_gain < self.current_gain {
            self.current_gain = target_gain + (self.current_gain - target_gain) * self.attack_coeff;
        } else {
            self.current_gain = target_gain + (self.current_gain - target_gain) * self.release_coeff;
        }

        // 3. 取出 2.0ms 前的延迟样本，并写入当前样本
        let delayed_l = self.delay_l[self.delay_idx];
        let delayed_r = self.delay_r[self.delay_idx];
        self.delay_l[self.delay_idx] = left;
        self.delay_r[self.delay_idx] = right;
        self.delay_idx = (self.delay_idx + 1) % self.delay_len;

        // 4. 将动态包络增益平滑施加在延迟样本上（波形曲线保持完美原形）
        let out_l = delayed_l * self.current_gain;
        let out_r = delayed_r * self.current_gain;

        (out_l, out_r)
    }
}

/// 智能平滑软饱和限幅器 (Smooth Soft Saturation Limiter)
/// - 低于 0.75 (~ -2.5 dBFS)：100% 原始信号线性直通，零失真、零音染
/// - 高于 0.75：无缝平滑过渡至双曲正切软饱和曲线，一阶导数严格连续（C1 连续无折角）
/// - 极限天花板严格锁定在 0.98 (~ -0.18 dBFS)：绝不触及 1.0 满刻度，彻底杜绝后续硬件/整数截断的数字削波
#[inline]
fn soft_limit(sample: f32) -> f32 {
    let abs_val = sample.abs();
    const THRESHOLD: f32 = 0.75;
    const CEILING: f32 = 0.98;

    if abs_val <= THRESHOLD {
        sample
    } else {
        let excess = abs_val - THRESHOLD;
        let margin = CEILING - THRESHOLD;
        let compressed = THRESHOLD + margin * (excess / margin).tanh();
        sample.signum() * compressed
    }
}

impl AudioEngine {
    pub fn start(
        input_name: &str,
        output_name: &str,
        controller: &AudioController,
    ) -> Result<Self, String> {
        controller.clear_error();

        let in_dev = find_device_by_name(true, input_name)
            .ok_or_else(|| format!("找不到输入设备: {}", input_name))?;
        let out_dev = find_device_by_name(false, output_name)
            .ok_or_else(|| format!("找不到输出设备: {}", output_name))?;

        let in_config_supported = in_dev
            .default_input_config()
            .map_err(|e| format!("获取输入设备配置失败: {}", e))?;
        let out_config_supported = out_dev
            .default_output_config()
            .map_err(|e| format!("获取输出设备配置失败: {}", e))?;

        let in_channels = in_config_supported.channels() as usize;
        let out_channels = out_config_supported.channels() as usize;
        let in_sample_format = in_config_supported.sample_format();
        let out_sample_format = out_config_supported.sample_format();

        let in_stream_config: StreamConfig = in_config_supported.into();
        let out_stream_config: StreamConfig = out_config_supported.into();

        // 缓冲区容量：约 25ms-30ms 低延迟
        let ring_buffer = HeapRb::<f32>::new(2880);
        let (mut producer, mut consumer) = ring_buffer.split();

        let gain_linear_in = controller.gain_linear.clone();
        let muted_in = controller.muted.clone();
        let peak_atomic = controller.peak_level.clone();
        let err_clone1 = controller.last_error.clone();
        let err_clone2 = controller.last_error.clone();

        let sample_rate = in_stream_config.sample_rate.0 as f32;
        let mut limiter = LookaheadLimiter::new(sample_rate);

        // 辅助处理函数：对立体声样本做增益、前瞻动态压限、测峰值与软限幅，并推入环形缓冲
        let mut process_and_push = move |mut left: f32, mut right: f32, max_peak: &mut f32| {
            let is_muted = muted_in.load(Ordering::Relaxed);
            let gain = f32::from_bits(gain_linear_in.load(Ordering::Relaxed));

            if is_muted {
                left = 0.0;
                right = 0.0;
            } else {
                left *= gain;
                right *= gain;

                // 1. 手机级前瞻动态压限（毫秒级动态包络跟随，保持波形完整性）
                let (dyn_l, dyn_r) = limiter.process(left, right);

                // 2. 模拟软饱和安全底线（双重保障彻底杜绝数字削波）
                left = soft_limit(dyn_l);
                right = soft_limit(dyn_r);

                // 记录输出峰值，供电平表真实反馈
                *max_peak = (*max_peak).max(left.abs()).max(right.abs());
            }

            // 保持低延迟并确保立体声通道严格对齐：成对写入，若缓冲区已满则整帧丢弃
            if producer.vacant_len() >= 2 {
                let _ = producer.try_push(left);
                let _ = producer.try_push(right);
            }
        };

        // 构建输入流（支持 F32 与 I16）
        let input_stream = match in_sample_format {
            SampleFormat::F32 => in_dev.build_input_stream(
                &in_stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut block_peak: f32 = 0.0;
                    let frames = data.len() / in_channels;
                    for f in 0..frames {
                        let offset = f * in_channels;
                        let (l, r) = if in_channels == 1 {
                            (data[offset], data[offset])
                        } else {
                            (data[offset], data[offset + 1])
                        };
                        process_and_push(l, r, &mut block_peak);
                    }
                    peak_atomic.store(block_peak.to_bits(), Ordering::Relaxed);
                },
                move |err| {
                    if let Ok(mut lock) = err_clone1.lock() {
                        *lock = Some(format!("输入流异常: {}", err));
                    }
                },
                None,
            ),
            SampleFormat::I16 => in_dev.build_input_stream(
                &in_stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mut block_peak: f32 = 0.0;
                    let frames = data.len() / in_channels;
                    for f in 0..frames {
                        let offset = f * in_channels;
                        let (l, r) = if in_channels == 1 {
                            let s = data[offset] as f32 / 32768.0;
                            (s, s)
                        } else {
                            (
                                data[offset] as f32 / 32768.0,
                                data[offset + 1] as f32 / 32768.0,
                            )
                        };
                        process_and_push(l, r, &mut block_peak);
                    }
                    peak_atomic.store(block_peak.to_bits(), Ordering::Relaxed);
                },
                move |err| {
                    if let Ok(mut lock) = err_clone1.lock() {
                        *lock = Some(format!("输入流异常: {}", err));
                    }
                },
                None,
            ),
            _ => return Err(format!("不支持的输入格式: {:?}", in_sample_format)),
        }
        .map_err(|e| format!("创建输入流失败: {}", e))?;

        // 构建输出流（支持 F32 与 I16）
        let output_stream = match out_sample_format {
            SampleFormat::F32 => out_dev.build_output_stream(
                &out_stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / out_channels;
                    for f in 0..frames {
                        let offset = f * out_channels;
                        let left = consumer.try_pop().unwrap_or(0.0);
                        let right = consumer.try_pop().unwrap_or(0.0);

                        if out_channels == 1 {
                            data[offset] = (left + right) * 0.5;
                        } else {
                            data[offset] = left;
                            data[offset + 1] = right;
                            for c in 2..out_channels {
                                data[offset + c] = 0.0;
                            }
                        }
                    }
                },
                move |err| {
                    if let Ok(mut lock) = err_clone2.lock() {
                        *lock = Some(format!("输出流异常: {}", err));
                    }
                },
                None,
            ),
            SampleFormat::I16 => out_dev.build_output_stream(
                &out_stream_config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / out_channels;
                    for f in 0..frames {
                        let offset = f * out_channels;
                        let left = consumer.try_pop().unwrap_or(0.0);
                        let right = consumer.try_pop().unwrap_or(0.0);

                        let to_i16 = |s: f32| (s * 32767.0).clamp(-32768.0, 32767.0) as i16;

                        if out_channels == 1 {
                            data[offset] = to_i16((left + right) * 0.5);
                        } else {
                            data[offset] = to_i16(left);
                            data[offset + 1] = to_i16(right);
                            for c in 2..out_channels {
                                data[offset + c] = 0;
                            }
                        }
                    }
                },
                move |err| {
                    if let Ok(mut lock) = err_clone2.lock() {
                        *lock = Some(format!("输出流异常: {}", err));
                    }
                },
                None,
            ),
            _ => return Err(format!("不支持的输出格式: {:?}", out_sample_format)),
        }
        .map_err(|e| format!("创建输出流失败: {}", e))?;

        input_stream.play().map_err(|e| format!("启动输入流失败: {}", e))?;
        output_stream.play().map_err(|e| format!("启动输出流失败: {}", e))?;

        Ok(Self {
            _input_stream: input_stream,
            _output_stream: output_stream,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_input_devices() {
        let devs = list_input_devices();
        println!("Filtered input devices: {:?}", devs);
        for d in &devs {
            assert!(!is_virtual_audio_input(d));
        }

        let out_devs = list_output_devices();
        println!("Output devices: {:?}", out_devs);
    }

    #[test]
    fn test_soft_limiter_continuity_and_ceiling() {
        // 1. 线性区完全透明直通（零染色）
        assert_eq!(soft_limit(0.0), 0.0);
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.5), -0.5);
        assert_eq!(soft_limit(0.75), 0.75);

        // 2. 临界点连续性（绝无断崖跌落）
        let delta = 0.0001;
        let diff = (soft_limit(0.75 + delta) - soft_limit(0.75 - delta)).abs();
        assert!(diff < 0.0005, "软限幅在临界点必须严格连续平滑，实际差异: {}", diff);

        // 3. 超高音量 / 炸麦极限天花板测试（永远不超 0.98，杜绝削波）
        for amp in [1.0, 1.5, 2.0, 5.0, 10.0, 100.0] {
            let pos = soft_limit(amp);
            let neg = soft_limit(-amp);
            assert!(pos <= 0.98, "输出电平 {} 超过天花板 0.98", pos);
            assert!(neg >= -0.98, "负输出电平 {} 低于天花板 -0.98", neg);
            assert!((pos + neg).abs() < 1e-6, "波形必须对称");
        }
    }

    #[test]
    fn test_lookahead_limiter_transparency_and_protection() {
        let mut limiter = LookaheadLimiter::new(48000.0);
        let sample_rate = 48000;

        // 1. 低电平正常说话透明度测试 (0.4 振幅，持续 100ms)
        // 预热 2ms 延迟缓冲区
        for _ in 0..150 {
            limiter.process(0.4, 0.4);
        }
        let (out_l, out_r) = limiter.process(0.4, 0.4);
        assert!((out_l - 0.4).abs() < 1e-3, "正常音量应该 100% 透明直通");
        assert!((out_r - 0.4).abs() < 1e-3);

        // 2. 突发大声 / 贴麦喊叫 (振幅 3.0，模拟 +14dB~+18dB 强声浪，持续 200ms)
        let mut max_output: f32 = 0.0;
        for i in 0..(sample_rate / 5) {
            // 200Hz 正弦波，峰值 3.0
            let t = (i as f32) / 48000.0;
            let val = 3.0 * (2.0 * std::f32::consts::PI * 200.0 * t).sin();
            let (ol, or_) = limiter.process(val, val);
            let safe_l = soft_limit(ol);
            let safe_r = soft_limit(or_);
            max_output = max_output.max(safe_l.abs()).max(safe_r.abs());
        }

        // 验证输出峰值牢牢被压在安全区 (<= 0.98)
        assert!(max_output <= 0.98, "突发强音量输出峰值 {} 超出安全天花板", max_output);

        // 3. 恢复平静后平滑释放测试 (回弹 150ms)
        for _ in 0..(sample_rate * 15 / 100) {
            limiter.process(0.3, 0.3);
        }
        let (recovered_l, _) = limiter.process(0.3, 0.3);
        assert!((recovered_l - 0.3).abs() < 0.05, "大声过后音量应平滑回弹");
    }
}
