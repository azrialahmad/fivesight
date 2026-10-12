//! WASAPI capture: the default microphone ("me") and system loopback ("others").
//!
//! Both are opened in shared, event-driven mode with `autoconvert`, so Windows hands us
//! 16 kHz mono f32 directly and no resampler is needed. Audio is delivered in exact
//! [`FRAME`]-sample frames (the size Silero VAD wants).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle,
    SampleType, StreamMode, WaveFormat,
};

pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME: usize = 512;

/// Event wait timeout. Loopback delivers no packets while nothing is playing, so on a
/// timeout we synthesize this much silence to keep the VAD clock moving.
const WAIT_MS: u32 = 100;
const SILENCE_SAMPLES_PER_TIMEOUT: usize = (SAMPLE_RATE as usize * WAIT_MS as usize) / 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Default microphone: the local user.
    Mic,
    /// Default playback device in loopback mode: everyone else in the call.
    Loopback,
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("wasapi: {0}")]
    Wasapi(String),
    #[error("capture thread: {0}")]
    Thread(String),
}

fn werr<E: std::fmt::Display>(e: E) -> AudioError {
    AudioError::Wasapi(e.to_string())
}

/// Running capture thread. Dropping it stops the stream.
pub struct Capture {
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
    pub device_name: String,
}

struct Session {
    client: AudioClient,
    reader: AudioCaptureClient,
    event: Handle,
}

fn open(source: Source) -> Result<(Session, String), AudioError> {
    initialize_mta().ok().map_err(werr)?;
    let enumerator = DeviceEnumerator::new().map_err(werr)?;
    let device_dir = match source {
        Source::Mic => Direction::Capture,
        Source::Loopback => Direction::Render,
    };
    let device = enumerator.get_default_device(&device_dir).map_err(werr)?;
    let name = device.get_friendlyname().unwrap_or_else(|_| "unknown".into());

    let mut client = device.get_iaudioclient().map_err(werr)?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE as usize, 1, None);
    let (_default, min_period) = client.get_device_period().map_err(werr)?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_period,
    };
    // For loopback the stream direction is Capture on a Render device.
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(werr)?;
    let event = client.set_get_eventhandle().map_err(werr)?;
    let reader = client.get_audiocaptureclient().map_err(werr)?;
    Ok((Session { client, reader, event }, name))
}

/// Emit as many whole frames as `samples` holds. Returns false if the receiver is gone.
fn emit_frames(
    samples: &mut Vec<f32>,
    tx: &SyncSender<Vec<f32>>,
    dropped: &AtomicU64,
) -> bool {
    let whole = samples.len() / FRAME * FRAME;
    for chunk in samples[..whole].chunks_exact(FRAME) {
        match tx.try_send(chunk.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    samples.drain(..whole);
    true
}

fn run(session: Session, tx: SyncSender<Vec<f32>>, stop: Arc<AtomicBool>, dropped: Arc<AtomicU64>) {
    let Session { client, reader, event } = session;
    let mut bytes: VecDeque<u8> = VecDeque::with_capacity(64 * 1024);
    let mut samples: Vec<f32> = Vec::with_capacity(FRAME * 8);

    if let Err(e) = client.start_stream() {
        tracing::error!("start_stream: {e}");
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = reader.read_from_device_to_deque(&mut bytes) {
            tracing::error!("read: {e}");
            break;
        }
        while bytes.len() >= 4 {
            let b = [
                bytes.pop_front().unwrap(),
                bytes.pop_front().unwrap(),
                bytes.pop_front().unwrap(),
                bytes.pop_front().unwrap(),
            ];
            samples.push(f32::from_le_bytes(b));
        }
        if event.wait_for_event(WAIT_MS).is_err() {
            samples.extend(std::iter::repeat(0.0).take(SILENCE_SAMPLES_PER_TIMEOUT));
        }
        if !emit_frames(&mut samples, &tx, &dropped) {
            break;
        }
    }
    let _ = client.stop_stream();
}

impl Capture {
    /// Start capturing. Frames of [`FRAME`] samples are sent to `tx`; if the channel is
    /// full the frame is dropped and counted in [`Capture::dropped_frames`].
    pub fn start(source: Source, tx: SyncSender<Vec<f32>>) -> Result<Self, AudioError> {
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, AudioError>>();

        let (stop2, dropped2) = (stop.clone(), dropped.clone());
        let join = thread::Builder::new()
            .name(format!("capture-{source:?}"))
            .spawn(move || match open(source) {
                Ok((session, name)) => {
                    let _ = ready_tx.send(Ok(name));
                    run(session, tx, stop2, dropped2);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| AudioError::Thread(e.to_string()))?;

        let device_name = ready_rx
            .recv()
            .map_err(|e| AudioError::Thread(e.to_string()))??;
        Ok(Self { stop, dropped, join: Some(join), device_name })
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Root-mean-square level of a block of samples.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn rms_of_silence_and_square_wave() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 100]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn emit_frames_keeps_remainder() {
        let (tx, rx) = sync_channel(8);
        let dropped = AtomicU64::new(0);
        let mut s = vec![0.25f32; FRAME * 2 + 100];
        assert!(emit_frames(&mut s, &tx, &dropped));
        assert_eq!(s.len(), 100);
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn full_channel_drops_and_counts() {
        let (tx, _rx) = sync_channel(1);
        let dropped = AtomicU64::new(0);
        let mut s = vec![0.0f32; FRAME * 3];
        assert!(emit_frames(&mut s, &tx, &dropped));
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn closed_channel_reports_stop() {
        let (tx, rx) = sync_channel::<Vec<f32>>(1);
        drop(rx);
        let dropped = AtomicU64::new(0);
        let mut s = vec![0.0f32; FRAME];
        assert!(!emit_frames(&mut s, &tx, &dropped));
    }
}
