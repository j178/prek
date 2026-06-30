// MIT License
//
// Copyright (c) 2023 Astral Software Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{LazyLock, Mutex};

use anstream::eprintln;
use owo_colors::OwoColorize;

/// Whether user-facing warnings are enabled.
pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static WARNINGS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Enable user-facing warnings.
pub fn enable() {
    ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Disable user-facing warnings.
pub fn disable() {
    ENABLED.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Whether user-facing warnings are currently enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Emit a user-facing warning.
pub fn warn(message: fmt::Arguments<'_>) {
    if is_enabled() {
        emit(message.to_string());
    }
}

/// Emit a user-facing warning once, with uniqueness determined by the content of the message.
pub fn warn_once(message: fmt::Arguments<'_>) {
    if !is_enabled() {
        return;
    }

    let message = message.to_string();
    let should_emit = match WARNINGS.lock() {
        Ok(mut states) => states.insert(message.clone()),
        Err(_) => false,
    };
    if should_emit {
        emit(message);
    }
}

fn emit(message: String) {
    crate::cli::reporter::suspend(move || {
        eprintln!(
            "{}{} {}",
            "warning".yellow().bold(),
            ":".bold(),
            message.bold()
        );
    });
}

/// Warn a user, if warnings are enabled.
#[macro_export]
macro_rules! warn_user {
    ($($arg:tt)*) => {
        if $crate::warnings::is_enabled() {
            $crate::warnings::warn(format_args!($($arg)*));
        }
    };
}

/// Warn a user once, if warnings are enabled, with uniqueness determined by the content of the
/// message.
#[macro_export]
macro_rules! warn_user_once {
    ($($arg:tt)*) => {
        if $crate::warnings::is_enabled() {
            $crate::warnings::warn_once(format_args!($($arg)*));
        }
    };
}
