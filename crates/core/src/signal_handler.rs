//! Signal handling for graceful shutdown.
//!
//! Provides graceful Ctrl-C (SIGINT) handling for long-running operations.

use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Check if a shutdown has been requested.
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Request a shutdown.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Reset the shutdown flag (for testing).
#[cfg(test)]
pub fn reset_shutdown() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

/// Setup signal handler for graceful shutdown.
/// Returns a guard that will clean up when dropped.
pub fn setup_signal_handler() -> SignalGuard {
    #[cfg(unix)]
    {
        use signal_hook::{consts::SIGINT, iterator::Signals};

        let mut signals = Signals::new([SIGINT]).expect("Failed to register signal handler");
        let handle = signals.handle();

        let thread = std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                eprintln!("\n  Shutdown requested (Ctrl-C)...");
                request_shutdown();
            }
        });

        SignalGuard {
            #[cfg(unix)]
            handle: Some((handle, thread)),
        }
    }

    #[cfg(windows)]
    {
        // Windows uses SetConsoleCtrlHandler
        use winapi::um::consoleapi::SetConsoleCtrlHandler;
        use winapi::um::wincon::CTRL_C_EVENT;

        extern "system" fn ctrl_handler(_: u32) -> i32 {
            eprintln!("\n  Shutdown requested (Ctrl-C)...");
            request_shutdown();
            1
        }

        unsafe {
            if SetConsoleCtrlHandler(Some(ctrl_handler), 1) == 0 {
                eprintln!("  Warning: Failed to set Ctrl-C handler");
            }
        }

        SignalGuard {
            #[cfg(unix)]
            handle: None,
        }
    }
}

/// Guard that cleans up signal handlers when dropped.
pub struct SignalGuard {
    #[cfg(unix)]
    handle: Option<(signal_hook::iterator::Handle, std::thread::JoinHandle<()>)>,
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some((handle, thread)) = self.handle.take() {
            handle.close();
            let _ = thread.join();
        }
    }
}

/// Check for shutdown and exit if requested.
/// Call this periodically in long-running operations.
pub fn check_shutdown() -> Result<(), ShutdownError> {
    if is_shutdown_requested() {
        Err(ShutdownError::Requested)
    } else {
        Ok(())
    }
}

/// Error type for shutdown requests.
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("Shutdown requested by user")]
    Requested,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_flag() {
        reset_shutdown();
        assert!(!is_shutdown_requested());

        request_shutdown();
        assert!(is_shutdown_requested());

        reset_shutdown();
        assert!(!is_shutdown_requested());
    }

    #[test]
    fn test_check_shutdown() {
        reset_shutdown();
        assert!(check_shutdown().is_ok());

        request_shutdown();
        assert!(check_shutdown().is_err());

        reset_shutdown();
    }
}
