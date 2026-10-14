//! Log file in `<app data>\logs\fivesight.log`, plus stderr for `scripts\dev.bat`.
//!
//! The previous run's log is kept as `fivesight.1.log`, so a problem can still be read after
//! restarting the app. Transcript text only shows up at debug level for dropped lines.

use std::fs::{self, File};
use std::path::Path;
use std::sync::Mutex;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

pub fn init(data_dir: &Path) {
    let dir = data_dir.join("logs");
    let file = fs::create_dir_all(&dir).and_then(|_| {
        let current = dir.join("fivesight.log");
        let _ = fs::rename(&current, dir.join("fivesight.1.log"));
        File::create(current)
    });

    // Our own crates at debug, everything else (tauri, wry, ...) at info.
    let filter = Targets::new()
        .with_default(LevelFilter::INFO)
        .with_target("fivesight_app_lib", LevelFilter::DEBUG)
        .with_target("fs_pipeline", LevelFilter::DEBUG);
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_filter(filter.clone());
    let (file, open_err) = match file {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(e)),
    };
    let file_layer =
        file.map(|f| tracing_subscriber::fmt::layer().with_ansi(false).with_writer(Mutex::new(f)).with_filter(filter));
    if tracing_subscriber::registry().with(stderr).with(file_layer).try_init().is_err() {
        return; // already set up (tests)
    }
    if let Some(e) = open_err {
        tracing::warn!("couldn't open the log file in {}: {e}", dir.display());
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
    tracing::info!("fivesight {} starting", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn writes_to_the_log_file_and_keeps_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("fivesight-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("logs").join("fivesight.log"), "last run\n").unwrap();

        super::init(&dir);
        tracing::debug!("echo check");

        let logs = dir.join("logs");
        assert_eq!(std::fs::read_to_string(logs.join("fivesight.1.log")).unwrap(), "last run\n");
        let now = std::fs::read_to_string(logs.join("fivesight.log")).unwrap();
        assert!(now.contains("starting") && now.contains("echo check"), "{now}");
    }
}
