//! macOS: the microphone and talk mode's stream through Core Audio, files
//! through `afplay`. Each device stream lives on a thread of its own,
//! since Core Audio streams cannot move between threads.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc as sync_mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use super::{Chunks, Kind, Output, Resampler, downmix};

/// How long the device may take to start, e.g. while macOS asks the user
/// whether the terminal may use the microphone.
const START_TIMEOUT: Duration = Duration::from_secs(30);

pub fn file_player(path: &Path, _kind: Kind) -> Result<(String, tokio::process::Command)> {
    let mut command = tokio::process::Command::new("afplay");
    command.arg(path);
    Ok(("afplay".to_owned(), command))
}

pub fn capture(rate: u32, tx: Chunks, stop: Arc<AtomicBool>) -> Result<()> {
    run_stream(stop, move || open_input(rate, tx)).map(|_| ())
}

pub fn speaker(rate: u32) -> Result<Box<dyn Output>> {
    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let shared = queue.clone();
    let device_rate = run_stream(stop.clone(), move || open_output(shared))?;
    Ok(Box::new(DeviceOutput {
        queue,
        resampler: Resampler::new(rate, device_rate),
        stop,
    }))
}

/// Opens a stream on a thread that keeps it until `stop`; returns what
/// opening it returned besides the stream.
fn run_stream<T: Send + 'static>(
    stop: Arc<AtomicBool>,
    open: impl FnOnce() -> Result<(cpal::Stream, T)> + Send + 'static,
) -> Result<T> {
    let (ready_tx, ready) = sync_mpsc::channel();
    std::thread::spawn(move || {
        let (stream, value) = match open() {
            Ok(opened) => opened,
            Err(err) => {
                let _ = ready_tx.send(Err(err));
                return;
            }
        };
        let _ = ready_tx.send(Ok(value));
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(stream);
    });
    match ready.recv_timeout(START_TIMEOUT) {
        Ok(result) => result,
        Err(_) => bail!(
            "the audio device did not start: {}",
            super::permission_hint()
        ),
    }
}

fn open_input(rate: u32, tx: Chunks) -> Result<(cpal::Stream, ())> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no microphone: connect one or choose an input in System Settings › Sound")?;
    let supported = device
        .default_input_config()
        .map_err(|e| anyhow!("the microphone reports no format: {e}"))?;
    let channels = supported.channels();
    let mut resampler = Resampler::new(supported.sample_rate(), rate);
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut deliver: impl FnMut(Vec<i16>) + Send + 'static,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample,
        i16: FromSample<T>,
    {
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &_| deliver(data.iter().map(|s| s.to_sample()).collect()),
                |err| tracing::warn!("microphone: {err}"),
                None,
            )
            .map_err(|e| anyhow!("cannot open the microphone: {e}"))
    }
    let deliver = move |samples: Vec<i16>| {
        let mono = resampler.process(&downmix(&samples, channels));
        if !mono.is_empty() {
            let _ = tx.send(mono);
        }
    };
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, deliver),
        SampleFormat::I16 => build::<i16>(&device, config, deliver),
        SampleFormat::I32 => build::<i32>(&device, config, deliver),
        SampleFormat::U16 => build::<u16>(&device, config, deliver),
        other => bail!("the microphone's sample format ({other}) is not supported"),
    };
    // A refused permission shows here, or as a recording of silence.
    let stream = stream.map_err(|e| anyhow!("{e:#}; {}", super::permission_hint()))?;
    stream.play().map_err(|e| {
        anyhow!(
            "cannot start the microphone: {e}; {}",
            super::permission_hint()
        )
    })?;
    Ok((stream, ()))
}

fn open_output(queue: Arc<Mutex<VecDeque<f32>>>) -> Result<(cpal::Stream, u32)> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no speaker: choose an output in System Settings › Sound")?;
    let supported = device
        .default_output_config()
        .map_err(|e| anyhow!("the speaker reports no format: {e}"))?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        queue: Arc<Mutex<VecDeque<f32>>>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &_| {
                    let mut queue = queue.lock().unwrap();
                    for frame in data.chunks_mut(channels.max(1)) {
                        let value = T::from_sample(queue.pop_front().unwrap_or(0.0));
                        frame.fill(value);
                    }
                },
                |err| tracing::warn!("speaker: {err}"),
                None,
            )
            .map_err(|e| anyhow!("cannot open the speaker: {e}"))
    }
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, channels, queue)?,
        SampleFormat::I16 => build::<i16>(&device, config, channels, queue)?,
        SampleFormat::I32 => build::<i32>(&device, config, channels, queue)?,
        SampleFormat::U16 => build::<u16>(&device, config, channels, queue)?,
        other => bail!("the speaker's sample format ({other}) is not supported"),
    };
    stream
        .play()
        .map_err(|e| anyhow!("cannot start the speaker: {e}"))?;
    Ok((stream, rate))
}

/// Talk mode's stream on the default output device.
struct DeviceOutput {
    queue: Arc<Mutex<VecDeque<f32>>>,
    resampler: Resampler,
    stop: Arc<AtomicBool>,
}

impl Output for DeviceOutput {
    fn write(&mut self, samples: &[i16]) {
        let resampled = self.resampler.process(samples);
        let mut queue = self.queue.lock().unwrap();
        queue.extend(resampled.iter().map(|s| f32::from(*s) / 32_768.0));
    }

    fn clear(&mut self) {
        self.queue.lock().unwrap().clear();
    }
}

impl Drop for DeviceOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
