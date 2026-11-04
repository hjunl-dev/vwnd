// ============================================================
// log
// - one place for diagnostic output, with levels
// - output: stderr, and OutputDebugStringW (debugger output window / DebugView), which
//   still works in a windows-subsystem build that has no console
// - usage: log_info!("app", "opened {uri}") -> "[app] info: opened ..."
// ============================================================

use std::fmt::Arguments;

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_core::HSTRING;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// Lines below this level are dropped (debug builds keep everything).
pub const MIN_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Debug
} else {
    Level::Info
};

/// Called by the log_* macros; `target` names the module ("app", "webview", ...).
pub fn write(level: Level, target: &str, args: Arguments) {
    if level < MIN_LEVEL {
        return;
    }
    let line = format!("[{target}] {}: {args}", level.as_str());
    eprintln!("{line}");
    // the debugger shows the text as-is: end each line with a newline
    unsafe { OutputDebugStringW(&HSTRING::from(format!("{line}\n"))) };
}

macro_rules! log_debug {
    ($target:expr, $($arg:tt)+) => {
        $crate::gui::log::write($crate::gui::log::Level::Debug, $target, format_args!($($arg)+))
    };
}

macro_rules! log_info {
    ($target:expr, $($arg:tt)+) => {
        $crate::gui::log::write($crate::gui::log::Level::Info, $target, format_args!($($arg)+))
    };
}

macro_rules! log_warn {
    ($target:expr, $($arg:tt)+) => {
        $crate::gui::log::write($crate::gui::log::Level::Warn, $target, format_args!($($arg)+))
    };
}

macro_rules! log_error {
    ($target:expr, $($arg:tt)+) => {
        $crate::gui::log::write($crate::gui::log::Level::Error, $target, format_args!($($arg)+))
    };
}

// path-based access: `use crate::gui::log::{log_info, ...}` from any module
#[allow(unused_imports)]
pub(crate) use {log_debug, log_error, log_info, log_warn};