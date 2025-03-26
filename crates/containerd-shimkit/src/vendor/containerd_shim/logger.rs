/*
   Copyright The containerd Authors.

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

//! VENDORED CODE FROM containerd-shim v0.8.0
//!
//! This file is vendored from the containerd-shim crate and should be replaced
//! with the upstream version when a new release is available.
//!
//! Source: <https://github.com/containerd/rust-extensions/blob/main/crates/containerd-shim/src/logger.rs>

use std::borrow::BorrowMut;
use std::fmt::Write as fmtwrite;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use containerd_shim::error::Error;
use log::kv::{self, Visitor};
use log::{Metadata, Record};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[cfg(windows)]
use crate::vendor::containerd_shim::sys::windows::NamedPipeLogger;

pub const LOG_ENV: &str = "RUST_LOG";

pub struct FifoLogger {
    file: Mutex<File>,
}

impl FifoLogger {
    #[allow(dead_code)]
    pub fn new() -> Result<FifoLogger, io::Error> {
        Self::with_path("log")
    }

    #[allow(dead_code)]
    pub fn with_path<P: AsRef<Path>>(path: P) -> Result<FifoLogger, io::Error> {
        let f = OpenOptions::new()
            .write(true)
            .read(false)
            .create(false)
            .open(path)?;

        Ok(FifoLogger {
            file: Mutex::new(f),
        })
    }
}

pub(crate) struct SimpleWriteVisitor {
    key_values: String,
}

impl<'kvs> Visitor<'kvs> for SimpleWriteVisitor {
    fn visit_pair(&mut self, k: kv::Key<'kvs>, v: kv::Value<'kvs>) -> Result<(), kv::Error> {
        write!(&mut self.key_values, " {}=\"{}\"", k, v)?;
        Ok(())
    }
}

impl SimpleWriteVisitor {
    pub(crate) fn new() -> SimpleWriteVisitor {
        SimpleWriteVisitor {
            key_values: String::new(),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.key_values
    }
}

impl log::Log for FifoLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut guard = self.file.lock().unwrap();

            // collect key_values but don't fail if error parsing
            let mut writer = SimpleWriteVisitor::new();
            let _ = record.key_values().visit(&mut writer);

            // The logger server may have temporarily shutdown, ignore the error instead of panic.
            //
            // Manual for pipe/FIFO: https://man7.org/linux/man-pages/man7/pipe.7.html
            // If all file descriptors referring to the read end of a pipe have been closed, then
            // a write(2) will cause a SIGPIPE signal to be generated for the calling process.
            // If the calling process is ignoring this signal, then write(2) fails with the error
            // EPIPE.
            let _ = writeln!(
                guard.borrow_mut(),
                "time=\"{}\" level={}{} msg=\"{}\"\n",
                rfc3339_formatted(),
                record.level().as_str().to_lowercase(),
                writer.as_str(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        // The logger server may have temporarily shutdown, ignore the error instead of panic.
        let _ = self.file.lock().unwrap().sync_all();
    }
}

pub fn init(
    debug: bool,
    default_log_level: &str,
    _namespace: &str,
    _id: &str,
) -> Result<(), Error> {
    #[cfg(unix)]
    let logger =
        FifoLogger::new().map_err(containerd_shim::io_error!(e, "failed to init logger"))?;

    // Containerd on windows expects the log to be a named pipe in the format of \\.\pipe\containerd-<namespace>-<id>-log
    // There is an assumption that there is always only one client connected which is containerd.
    // If there is a restart of containerd then logs during that time period will be lost.
    //
    // https://github.com/containerd/containerd/blob/v1.7.0/runtime/v2/shim_windows.go#L77
    // https://github.com/microsoft/hcsshim/blob/5871d0c4436f131c377655a3eb09fc9b5065f11d/cmd/containerd-shim-runhcs-v1/serve.go#L132-L137
    #[cfg(windows)]
    let logger = NamedPipeLogger::new(_namespace, _id)
        .map_err(containerd_shim::io_error!(e, "failed to init logger"))?;

    configure_logging_level(debug, default_log_level);
    log::set_boxed_logger(Box::new(logger))?;
    Ok(())
}

fn configure_logging_level(debug: bool, default_log_level: &str) {
    let debug_level = std::env::var(LOG_ENV).unwrap_or(default_log_level.to_string());
    let debug_level = log::LevelFilter::from_str(&debug_level).unwrap_or(log::LevelFilter::Info);
    let level = if debug && log::LevelFilter::Debug > debug_level {
        log::LevelFilter::Debug
    } else {
        debug_level
    };
    log::set_max_level(level);
}

pub(crate) fn rfc3339_formatted() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or(OffsetDateTime::now_utc().to_string())
}
