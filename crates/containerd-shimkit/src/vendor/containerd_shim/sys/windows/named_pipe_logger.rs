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

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{Metadata, Record};
use mio::windows::NamedPipe;
use mio::{Events, Interest, Poll, Token};

use crate::vendor::containerd_shim::logger;

pub struct NamedPipeLogger {
    current_connection: Arc<Mutex<NamedPipe>>,
}

impl NamedPipeLogger {
    pub fn new(namespace: &str, id: &str) -> Result<NamedPipeLogger, io::Error> {
        let pipe_name = format!("\\\\.\\pipe\\containerd-shim-{}-{}-log", namespace, id);
        let mut pipe_server = NamedPipe::new(pipe_name).unwrap();
        let mut poll = Poll::new().unwrap();
        poll.registry()
            .register(
                &mut pipe_server,
                Token(0),
                Interest::READABLE | Interest::WRITABLE,
            )
            .unwrap();

        let current_connection = Arc::new(Mutex::new(pipe_server));
        let server_connection = current_connection.clone();
        let logger = NamedPipeLogger { current_connection };

        thread::spawn(move || {
            let mut events = Events::with_capacity(128);
            loop {
                poll.poll(&mut events, None).unwrap();

                for event in events.iter() {
                    if event.is_writable() {
                        match server_connection.lock().unwrap().connect() {
                            Ok(()) => {}
                            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                                // this would block just keep processing
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // this would block just keep processing
                            }
                            Err(e) => {
                                panic!("Error connecting to client: {}", e);
                            }
                        };
                    }
                    if event.is_readable() {
                        server_connection.lock().unwrap().disconnect().unwrap();
                    }
                }
            }
        });

        Ok(logger)
    }
}

impl log::Log for NamedPipeLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // collect key_values but don't fail if error parsing
            let mut writer = logger::SimpleWriteVisitor::new();
            let _ = record.key_values().visit(&mut writer);

            let kvs = logger::KEY_VALUES.lock().unwrap().clone();

            let message = format!(
                "time=\"{}\" level={}{}{} msg=\"{}\"\n",
                logger::rfc3339_formatted(),
                record.level().as_str().to_lowercase(),
                kvs,
                writer.as_str(),
                record.args()
            );

            match self
                .current_connection
                .lock()
                .unwrap()
                .write_all(message.as_bytes())
            {
                Ok(_) => {}
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                    // this would block just keep processing
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // this would block just keep processing
                }
                Err(e) if e.raw_os_error() == Some(536) => {
                    // no client connected
                }
                Err(e) if e.raw_os_error() == Some(232) => {
                    // client was connected but is in process of shutting down
                }
                Err(e) => {
                    panic!("Error writing to client: {}", e)
                }
            }
        }
    }

    fn flush(&self) {
        _ = self.current_connection.lock().unwrap().flush();
    }
}
