
pub mod matrix;

use std::sync::Arc;
use lazy_static::lazy_static;
use tokio::{
    task::yield_now,
    time::{sleep, Duration, Instant},
    signal::ctrl_c,
    sync::RwLock,
    select,
};

lazy_static! {
    pub static ref CANCELED: Arc<RwLock<bool>> = Arc::new(RwLock::new(false));
}

// Sleep handler that allows for graceful termination so reports can be generated
pub async fn task_sleep(delay: f64, allow_cancel: bool) {
    let start = Instant::now();

    // Task sleeping hangs goose termination for long sleep durations, so its needed
    // frequently poll if sleep should be canceled.
    while start.elapsed() < Duration::from_secs_f64(delay) {
        if allow_cancel {
            // Drop lock after checking canceled status
            if *CANCELED.read().await {
                return;
            }
        }

        let remaining_duration = Duration::from_secs_f64(delay) - start.elapsed();
        // Sleep in 1s increments max to share resources with other users
        let sleep_duration = Duration::from_secs_f64(1.0).min(remaining_duration);

        select! {
            _ = sleep(sleep_duration) => { yield_now().await; },
            _ = ctrl_c() => { *CANCELED.write().await = true; },
        };
    }
}
