use cpal::{
    FromSample, HostId, Sample, SampleFormat,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};
use rustfft::{FftPlanner, num_complex::Complex32};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub const SPECTRUM_BANDS: usize = 24;
const FFT_SIZE: usize = 2048;
const RING_CAPACITY: usize = FFT_SIZE * 8;
const MIN_FREQUENCY: f32 = 60.0;
const MAX_FREQUENCY: f32 = 12_000.0;

pub struct SpectrumCapture {
    _stream: cpal::Stream,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SpectrumCapture {
    pub fn start() -> Result<(Self, Receiver<[f32; SPECTRUM_BANDS]>), String> {
        let host = cpal::host_from_id(HostId::PipeWire).map_err(|error| error.to_string())?;
        let device = host
            .devices()
            .map_err(|error| error.to_string())?
            .find(|device| {
                device.supports_input()
                    && device.supports_output()
                    && device
                        .id()
                        .is_ok_and(|id| id.to_string().ends_with("sink_default"))
            })
            .ok_or_else(|| "PipeWire has no capture-capable default sink".to_owned())?;
        let supported = device
            .default_input_config()
            .map_err(|error| error.to_string())?;
        let sample_rate = supported.sample_rate() as f32;
        let channels = usize::from(supported.channels());
        let ring = HeapRb::<f32>::new(RING_CAPACITY);
        let (producer, consumer) = ring.split();
        let (sender, receiver) = mpsc::channel();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, supported.into(), channels, producer),
            SampleFormat::I16 => build_stream::<i16>(&device, supported.into(), channels, producer),
            SampleFormat::I32 => build_stream::<i32>(&device, supported.into(), channels, producer),
            format => return Err(format!("unsupported PipeWire sample format: {format}")),
        }
        .map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = thread::spawn(move || {
            analyze_samples(consumer, sample_rate, sender, &worker_running);
        });
        Ok((
            Self {
                _stream: stream,
                running,
                worker: Some(worker),
            },
            receiver,
        ))
    }
}

impl Drop for SpectrumCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut producer: HeapProd<f32>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: Sample + cpal::SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |samples: &[T], _| {
            for frame in samples.chunks_exact(channels) {
                let mono =
                    frame.iter().copied().map(f32::from_sample).sum::<f32>() / channels as f32;
                let _ = producer.try_push(mono);
            }
        },
        |_| {},
        None,
    )
}

fn analyze_samples(
    mut consumer: HeapCons<f32>,
    sample_rate: f32,
    sender: Sender<[f32; SPECTRUM_BANDS]>,
    running: &AtomicBool,
) {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut samples = vec![0.0; FFT_SIZE];
    let mut spectrum = vec![Complex32::default(); FFT_SIZE];
    let mut smoothed = [0.0; SPECTRUM_BANDS];

    while running.load(Ordering::Relaxed) {
        if consumer.occupied_len() < FFT_SIZE {
            thread::sleep(Duration::from_millis(4));
            continue;
        }
        consumer.pop_slice(&mut samples);
        for (index, (sample, bin)) in samples.iter().zip(&mut spectrum).enumerate() {
            let window = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * index as f32 / (FFT_SIZE - 1) as f32).cos());
            *bin = Complex32::new(sample * window, 0.0);
        }
        fft.process(&mut spectrum);
        let levels = frequency_bands(&spectrum, sample_rate);
        for (smoothed, level) in smoothed.iter_mut().zip(levels) {
            *smoothed = if level > *smoothed {
                *smoothed * 0.25 + level * 0.75
            } else {
                (*smoothed * 0.82).max(level)
            };
        }
        if sender.send(smoothed).is_err() {
            break;
        }
    }
}

fn frequency_bands(spectrum: &[Complex32], sample_rate: f32) -> [f32; SPECTRUM_BANDS] {
    std::array::from_fn(|band| {
        let start_ratio = band as f32 / SPECTRUM_BANDS as f32;
        let end_ratio = (band + 1) as f32 / SPECTRUM_BANDS as f32;
        let start_hz = MIN_FREQUENCY * (MAX_FREQUENCY / MIN_FREQUENCY).powf(start_ratio);
        let end_hz = MIN_FREQUENCY * (MAX_FREQUENCY / MIN_FREQUENCY).powf(end_ratio);
        let start = ((start_hz * FFT_SIZE as f32 / sample_rate) as usize).max(1);
        let end = ((end_hz * FFT_SIZE as f32 / sample_rate) as usize)
            .max(start + 1)
            .min(FFT_SIZE / 2);
        let magnitude = spectrum[start..end]
            .iter()
            .map(|value| value.norm() * 2.0 / FFT_SIZE as f32)
            .fold(0.0_f32, f32::max);
        let decibels = 20.0 * magnitude.max(0.000_001).log10();
        ((decibels + 70.0) / 60.0).clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_produces_empty_bands() {
        assert_eq!(
            frequency_bands(&vec![Complex32::default(); FFT_SIZE], 48_000.0),
            [0.0; SPECTRUM_BANDS]
        );
    }

    #[test]
    fn tone_raises_a_frequency_band() {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let mut samples = (0..FFT_SIZE)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0;
                Complex32::new(phase.sin(), 0.0)
            })
            .collect::<Vec<_>>();
        fft.process(&mut samples);

        assert!(
            frequency_bands(&samples, 48_000.0)
                .iter()
                .any(|level| *level > 0.5)
        );
    }
}
