use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct ProgressUpdate {
    pub total_size: u64,
    pub uploaded: u64,
    pub progress: f64,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
    pub concurrent_chunks: usize,
    pub total_chunks: usize,
    pub completed_chunks: usize,
}

impl Debug for ProgressUpdate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Progress: {:.1}% ({} / {}) @ {} | ETA: {} | Chunks: {}/{} ({} active)",
            self.progress * 100.0,
            format_bytes(self.uploaded),
            format_bytes(self.total_size),
            format_speed(self.speed_bytes_per_sec),
            format_eta(self.eta_seconds),
            self.completed_chunks,
            self.total_chunks,
            self.concurrent_chunks,
        )
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn format_eta(eta_seconds: Option<u64>) -> String {
    match eta_seconds {
        None => "N/A".to_string(),
        Some(0) => "0s".to_string(),
        Some(secs) => {
            let hours = secs / 3600;
            let minutes = (secs % 3600) / 60;
            let seconds = secs % 60;

            if hours > 0 {
                format!("{}h {}m {}s", hours, minutes, seconds)
            } else if minutes > 0 {
                format!("{}m {}s", minutes, seconds)
            } else {
                format!("{}s", seconds)
            }
        }
    }
}

impl ProgressUpdate {
    pub fn new(
        total_size: u64,
        uploaded: u64,
        speed_bytes_per_sec: u64,
        concurrent_chunks: usize,
        total_chunks: usize,
        completed_chunks: usize,
    ) -> Self {
        let progress = if total_size > 0 {
            (uploaded as f64 / total_size as f64).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let eta_seconds = if speed_bytes_per_sec > 0 && uploaded < total_size {
            Some((total_size - uploaded) / speed_bytes_per_sec)
        } else {
            None
        };

        Self {
            total_size,
            uploaded,
            progress,
            speed_bytes_per_sec,
            eta_seconds,
            concurrent_chunks,
            total_chunks,
            completed_chunks,
        }
    }
}

pub trait ProgressCallback: Send + Sync {
    fn on_progress(&self, update: ProgressUpdate);
}

pub struct NoOpProgress;

impl ProgressCallback for NoOpProgress {
    fn on_progress(&self, _update: ProgressUpdate) {}
}

impl<T: ProgressCallback> ProgressCallback for Arc<T> {
    fn on_progress(&self, update: ProgressUpdate) {
        (**self).on_progress(update)
    }
}

struct SpeedCalculator {
    samples: Vec<(Instant, u64)>,
    window_duration: Duration,
}

impl SpeedCalculator {
    fn new() -> Self {
        Self {
            samples: Vec::with_capacity(32),
            window_duration: Duration::from_secs(10),
        }
    }

    fn record_and_calculate(&mut self, total_bytes: u64) -> u64 {
        let now = Instant::now();
        self.samples.push((now, total_bytes));

        let cutoff = now - self.window_duration;
        self.samples.retain(|(t, _)| *t >= cutoff);

        if self.samples.len() >= 2 {
            let (oldest_time, oldest_bytes) = self.samples.first().unwrap();
            let elapsed = now.duration_since(*oldest_time);
            if elapsed.as_millis() > 0 {
                let bytes_diff = total_bytes.saturating_sub(*oldest_bytes);
                return (bytes_diff as f64 / elapsed.as_secs_f64()) as u64;
            }
        }

        0
    }
}

pub struct ProgressTracker {
    total_size: u64,
    uploaded_bytes: AtomicU64,
    active_chunks: AtomicU64,
    total_chunks: usize,
    completed_chunks: AtomicU64,
    speed_calc: RwLock<SpeedCalculator>,
    cached_speed: AtomicU64,
}

impl ProgressTracker {
    pub fn new(total_size: u64, total_chunks: usize) -> Arc<Self> {
        Arc::new(Self {
            total_size,
            uploaded_bytes: AtomicU64::new(0),
            active_chunks: AtomicU64::new(0),
            total_chunks,
            completed_chunks: AtomicU64::new(0),
            speed_calc: RwLock::new(SpeedCalculator::new()),
            cached_speed: AtomicU64::new(0),
        })
    }

    pub fn start_chunk(&self) {
        self.active_chunks.fetch_add(1, Ordering::SeqCst);
    }

    pub fn complete_chunk(&self) {
        self.active_chunks.fetch_sub(1, Ordering::SeqCst);
        self.completed_chunks.fetch_add(1, Ordering::SeqCst);
    }

    pub fn add_bytes(&self, bytes: u64) {
        self.uploaded_bytes.fetch_add(bytes, Ordering::SeqCst);
    }

    pub fn reset_chunk_bytes(&self, bytes: u64) {
        self.uploaded_bytes.fetch_sub(bytes, Ordering::SeqCst);
    }

    pub fn total_uploaded(&self) -> u64 {
        self.uploaded_bytes.load(Ordering::SeqCst)
    }

    pub async fn create_update(&self) -> ProgressUpdate {
        let total_uploaded = self.total_uploaded();

        let speed = {
            let mut calc = self.speed_calc.write().await;
            let speed = calc.record_and_calculate(total_uploaded);
            self.cached_speed.store(speed, Ordering::SeqCst);
            speed
        };

        ProgressUpdate::new(
            self.total_size,
            total_uploaded,
            speed,
            self.active_chunks.load(Ordering::SeqCst) as usize,
            self.total_chunks,
            self.completed_chunks.load(Ordering::SeqCst) as usize,
        )
    }
}
