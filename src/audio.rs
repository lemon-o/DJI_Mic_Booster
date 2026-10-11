use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
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

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut list = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for dev in devices {
            if let Ok(name) = dev.name() {
                list.push(name);
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

        // 辅助处理函数：对立体声样本做增益、测峰值与软限幅，并推入环形缓冲
        let mut process_and_push = move |mut left: f32, mut right: f32, max_peak: &mut f32| {
            let is_muted = muted_in.load(Ordering::Relaxed);
            let gain = f32::from_bits(gain_linear_in.load(Ordering::Relaxed));

            if is_muted {
                left = 0.0;
                right = 0.0;
            } else {
                left *= gain;
                right *= gain;

                *max_peak = (*max_peak).max(left.abs()).max(right.abs());

                // 智能软限幅防爆音 (Soft Limiter)
                if left.abs() > 0.85 {
                    left = (left * 0.9).tanh() / 0.9;
                }
                if right.abs() > 0.85 {
                    right = (right * 0.9).tanh() / 0.9;
                }
            }

            // 保持低延迟：如满则丢弃最早数据或放弃写入
            let _ = producer.try_push(left);
            let _ = producer.try_push(right);
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
