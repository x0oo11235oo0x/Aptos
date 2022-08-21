// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

//! Implementation of writing logs to both local printers (e.g. stdout) and remote loggers
//! (e.g. Logstash)

use crate::{
    counters::{
        PROCESSED_STRUCT_LOG_COUNT, SENT_STRUCT_LOG_BYTES, SENT_STRUCT_LOG_COUNT,
        STRUCT_LOG_PARSE_ERROR_COUNT, STRUCT_LOG_QUEUE_ERROR_COUNT, STRUCT_LOG_SEND_ERROR_COUNT,
    },
    logger::Logger,
    struct_log::TcpWriter,
    Event, Filter, Key, Level, LevelFilter, Metadata,
};
use aptos_infallible::RwLock;
use backtrace::Backtrace;
use chrono::{SecondsFormat, Utc};
use once_cell::sync::Lazy;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::io::Stdout;
use std::{
    collections::BTreeMap,
    env, fmt,
    io::Write,
    str::FromStr,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
};
use strum_macros::EnumString;

const RUST_LOG: &str = "RUST_LOG";
const RUST_LOG_REMOTE: &str = "RUST_LOG_REMOTE";
const RUST_LOG_FORMAT: &str = "RUST_LOG_FORMAT";
/// Default size of log write channel, if the channel is full, logs will be dropped
pub const CHANNEL_SIZE: usize = 10000;
const NUM_SEND_RETRIES: u8 = 1;

#[derive(EnumString)]
#[strum(serialize_all = "lowercase")]
enum LogFormat {
    Json,
    Text,
}

/// A single log entry emitted by a logging macro with associated metadata
#[derive(Debug)]
pub struct LogEntry {
    metadata: Metadata,
    thread_name: Option<String>,
    /// The program backtrace taken when the event occurred. Backtraces
    /// are only supported for errors and must be configured.
    backtrace: Option<String>,
    hostname: Option<&'static str>,
    namespace: Option<&'static str>,
    timestamp: String,
    data: BTreeMap<Key, serde_json::Value>,
    message: Option<String>,
}

// implement custom serializer for LogEntry since we want to promote the `metadata.level` field into a top-level `level` field
// and prefix the remaining metadata attributes as `source.<metadata_field>` which can't be expressed with serde macros alone.
impl Serialize for LogEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LogEntry", 9)?;
        state.serialize_field("level", &self.metadata.level())?;
        state.serialize_field("source", &self.metadata)?;
        if let Some(thread_name) = &self.thread_name {
            state.serialize_field("thread_name", thread_name)?;
        }
        if let Some(hostname) = &self.hostname {
            state.serialize_field("hostname", hostname)?;
        }
        if let Some(namespace) = &self.namespace {
            state.serialize_field("namespace", namespace)?;
        }
        state.serialize_field("timestamp", &self.timestamp)?;
        if let Some(message) = &self.message {
            state.serialize_field("message", message)?;
        }
        if !&self.data.is_empty() {
            state.serialize_field("data", &self.data)?;
        }
        if let Some(backtrace) = &self.backtrace {
            state.serialize_field("backtrace", backtrace)?;
        }
        state.end()
    }
}

impl LogEntry {
    fn new(event: &Event, thread_name: Option<&str>, enable_backtrace: bool) -> Self {
        use crate::{Value, Visitor};

        struct JsonVisitor<'a>(&'a mut BTreeMap<Key, serde_json::Value>);

        impl<'a> Visitor for JsonVisitor<'a> {
            fn visit_pair(&mut self, key: Key, value: Value<'_>) {
                let v = match value {
                    Value::Debug(d) => serde_json::Value::String(format!("{:?}", d)),
                    Value::Display(d) => serde_json::Value::String(d.to_string()),
                    Value::Serde(s) => match serde_json::to_value(s) {
                        Ok(value) => value,
                        Err(e) => {
                            eprintln!("error serializing structured log: {}", e);
                            return;
                        }
                    },
                };

                self.0.insert(key, v);
            }
        }

        let metadata = *event.metadata();
        let thread_name = thread_name.map(ToOwned::to_owned);
        let message = event.message().map(fmt::format);

        static HOSTNAME: Lazy<Option<String>> = Lazy::new(|| {
            hostname::get()
                .ok()
                .and_then(|name| name.into_string().ok())
        });

        static NAMESPACE: Lazy<Option<String>> =
            Lazy::new(|| env::var("KUBERNETES_NAMESPACE").ok());

        let hostname = HOSTNAME.as_deref();
        let namespace = NAMESPACE.as_deref();

        let backtrace = if enable_backtrace && matches!(metadata.level(), Level::Error) {
            let mut backtrace = Backtrace::new();
            let mut frames = backtrace.frames().to_vec();
            if frames.len() > 3 {
                frames.drain(0..3); // Remove the first 3 unnecessary frames to simplify backtrace
            }
            backtrace = frames.into();
            Some(format!("{:?}", backtrace))
        } else {
            None
        };

        let mut data = BTreeMap::new();
        for schema in event.keys_and_values() {
            schema.visit(&mut JsonVisitor(&mut data));
        }

        Self {
            metadata,
            thread_name,
            backtrace,
            hostname,
            namespace,
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
            data,
            message,
        }
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn thread_name(&self) -> Option<&str> {
        self.thread_name.as_deref()
    }

    pub fn backtrace(&self) -> Option<&str> {
        self.backtrace.as_deref()
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname
    }

    pub fn namespace(&self) -> Option<&str> {
        self.namespace
    }

    pub fn timestamp(&self) -> &str {
        self.timestamp.as_str()
    }

    pub fn data(&self) -> &BTreeMap<Key, serde_json::Value> {
        &self.data
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// A builder for a `AptosData`, configures what, where, and how to write logs.
pub struct AptosDataBuilder {
    channel_size: usize,
    console_port: Option<u16>,
    enable_backtrace: bool,
    level: Level,
    remote_level: Level,
    address: Option<String>,
    printer: Option<Box<dyn Writer>>,
    is_async: bool,
    custom_format: Option<fn(&LogEntry) -> Result<String, fmt::Error>>,
}

impl AptosDataBuilder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            channel_size: CHANNEL_SIZE,
            console_port: Some(6669),
            enable_backtrace: false,
            level: Level::Info,
            remote_level: Level::Info,
            address: None,
            printer: Some(Box::new(StdoutWriter::new())),
            is_async: false,
            custom_format: None,
        }
    }

    pub fn address(&mut self, address: String) -> &mut Self {
        self.address = Some(address);
        self
    }

    pub fn enable_backtrace(&mut self) -> &mut Self {
        self.enable_backtrace = true;
        self
    }

    pub fn read_env(&mut self) -> &mut Self {
        if let Ok(address) = env::var("STRUCT_LOG_TCP_ADDR") {
            self.address(address);
        }
        self
    }

    pub fn level(&mut self, level: Level) -> &mut Self {
        self.level = level;
        self
    }

    pub fn remote_level(&mut self, level: Level) -> &mut Self {
        self.remote_level = level;
        self
    }

    pub fn channel_size(&mut self, channel_size: usize) -> &mut Self {
        self.channel_size = channel_size;
        self
    }

    pub fn printer(&mut self, printer: Box<dyn Writer + Send + Sync + 'static>) -> &mut Self {
        self.printer = Some(printer);
        self
    }

    pub fn console_port(&mut self, console_port: Option<u16>) -> &mut Self {
        self.console_port = console_port;
        self
    }
    pub fn is_async(&mut self, is_async: bool) -> &mut Self {
        self.is_async = is_async;
        self
    }

    pub fn custom_format(
        &mut self,
        format: fn(&LogEntry) -> Result<String, fmt::Error>,
    ) -> &mut Self {
        self.custom_format = Some(format);
        self
    }

    pub fn init(&mut self) {
        self.build();
    }

    pub fn build(&mut self) -> Arc<AptosData> {
        let filter = {
            let local_filter = {
                let mut filter_builder = Filter::builder();

                if env::var(RUST_LOG).is_ok() {
                    filter_builder.with_env(RUST_LOG);
                } else {
                    filter_builder.filter_level(self.level.into());
                }

                filter_builder.build()
            };
            let remote_filter = {
                let mut filter_builder = Filter::builder();

                if self.is_async && self.address.is_some() {
                    if env::var(RUST_LOG_REMOTE).is_ok() {
                        filter_builder.with_env(RUST_LOG_REMOTE);
                    } else if env::var(RUST_LOG).is_ok() {
                        filter_builder.with_env(RUST_LOG);
                    } else {
                        filter_builder.filter_level(self.remote_level.into());
                    }
                } else {
                    filter_builder.filter_level(LevelFilter::Off);
                }

                filter_builder.build()
            };

            FilterPair {
                local_filter,
                remote_filter,
            }
        };

        if let Ok(log_format) = env::var(RUST_LOG_FORMAT) {
            let log_format = LogFormat::from_str(&log_format).unwrap();
            self.custom_format = match log_format {
                LogFormat::Json => Some(json_format),
                LogFormat::Text => Some(text_format),
            }
        }

        let logger = if self.is_async {
            let (sender, receiver) = mpsc::sync_channel(self.channel_size);
            let logger = Arc::new(AptosData {
                enable_backtrace: self.enable_backtrace,
                sender: Some(sender),
                printer: None,
                filter: RwLock::new(filter),
                formatter: self.custom_format.take().unwrap_or(text_format),
            });
            let service = LoggerService {
                receiver,
                address: self.address.clone(),
                printer: self.printer.take(),
                facade: logger.clone(),
            };

            thread::spawn(move || service.run());
            logger
        } else {
            Arc::new(AptosData {
                enable_backtrace: self.enable_backtrace,
                sender: None,
                printer: self.printer.take(),
                filter: RwLock::new(filter),
                formatter: self.custom_format.take().unwrap_or(text_format),
            })
        };

        let console_port = if cfg!(feature = "aptos-console") {
            self.console_port
        } else {
            None
        };

        crate::logger::set_global_logger(logger.clone(), console_port);
        logger
    }
}

/// A combination of `Filter`s to control where logs are written
struct FilterPair {
    /// The local printer `Filter` to control what is logged in text output
    local_filter: Filter,
    /// The remote logging `Filter` to control what is sent to external logging
    remote_filter: Filter,
}

impl FilterPair {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.local_filter.enabled(metadata) || self.remote_filter.enabled(metadata)
    }
}

pub struct AptosData {
    enable_backtrace: bool,
    sender: Option<SyncSender<LoggerServiceEvent>>,
    printer: Option<Box<dyn Writer>>,
    filter: RwLock<FilterPair>,
    pub(crate) formatter: fn(&LogEntry) -> Result<String, fmt::Error>,
}

impl AptosData {
    pub fn builder() -> AptosDataBuilder {
        AptosDataBuilder::new()
    }

    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> AptosDataBuilder {
        Self::builder()
    }

    pub fn init_for_testing() {
        if env::var(RUST_LOG).is_err() {
            return;
        }

        Self::builder()
            .is_async(false)
            .enable_backtrace()
            .printer(Box::new(StdoutWriter::new()))
            .build();
    }

    pub fn set_filter(&self, filter: Filter) {
        self.filter.write().local_filter = filter;
    }

    pub fn set_remote_filter(&self, filter: Filter) {
        self.filter.write().remote_filter = filter;
    }

    fn send_entry(&self, entry: LogEntry) {
        if let Some(printer) = &self.printer {
            let s = (self.formatter)(&entry).expect("Unable to format");
            printer.write(s);
        }

        if let Some(sender) = &self.sender {
            if sender
                .try_send(LoggerServiceEvent::LogEntry(entry))
                .is_err()
            {
                STRUCT_LOG_QUEUE_ERROR_COUNT.inc();
            }
        }
    }
}

impl Logger for AptosData {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter.read().enabled(metadata)
    }

    fn record(&self, event: &Event) {
        let entry = LogEntry::new(
            event,
            ::std::thread::current().name(),
            self.enable_backtrace,
        );

        self.send_entry(entry)
    }

    fn flush(&self) {
        if let Some(sender) = &self.sender {
            let (oneshot_sender, oneshot_receiver) = mpsc::sync_channel(1);
            sender
                .send(LoggerServiceEvent::Flush(oneshot_sender))
                .unwrap();
            oneshot_receiver.recv().unwrap();
        }
    }
}

enum LoggerServiceEvent {
    LogEntry(LogEntry),
    Flush(SyncSender<()>),
}

/// A service for running a log listener, that will continually export logs through a local printer
/// or to a `AptosData` for external logging.
struct LoggerService {
    receiver: Receiver<LoggerServiceEvent>,
    address: Option<String>,
    printer: Option<Box<dyn Writer>>,
    facade: Arc<AptosData>,
}

impl LoggerService {
    pub fn run(mut self) {
        let mut writer = self.address.take().map(TcpWriter::new);

        for event in self.receiver {
            match event {
                LoggerServiceEvent::LogEntry(entry) => {
                    PROCESSED_STRUCT_LOG_COUNT.inc();

                    if let Some(printer) = &mut self.printer {
                        if self
                            .facade
                            .filter
                            .read()
                            .local_filter
                            .enabled(&entry.metadata)
                        {
                            let s = (self.facade.formatter)(&entry).expect("Unable to format");
                            printer.write_buferred(s);
                        }
                    }

                    if let Some(writer) = &mut writer {
                        if self
                            .facade
                            .filter
                            .read()
                            .remote_filter
                            .enabled(&entry.metadata)
                        {
                            Self::write_to_logstash(writer, entry);
                        }
                    }
                }
                LoggerServiceEvent::Flush(sender) => {
                    // This is just to notify the other side, the logger doesn't actually care if
                    // the listener is still listening
                    let _ = sender.send(());
                }
            }
        }
    }

    /// Writes a log line into json_lines logstash format, which has a newline at the end
    fn write_to_logstash(stream: &mut TcpWriter, entry: LogEntry) {
        let message = if let Ok(json) = json_format(&entry) {
            json
        } else {
            return;
        };
        let message = message + "\n";
        let bytes = message.as_bytes();
        let message_length = bytes.len();

        // Attempt to write the log up to NUM_SEND_RETRIES + 1, and then drop it
        // Each `write_all` call will attempt to open a connection if one isn't open
        let mut result = stream.write_all(bytes);
        for _ in 0..NUM_SEND_RETRIES {
            if result.is_ok() {
                break;
            } else {
                result = stream.write_all(bytes);
            }
        }

        if let Err(e) = result {
            STRUCT_LOG_SEND_ERROR_COUNT.inc();
            eprintln!(
                "[Logging] Error while sending data to logstash({}): {}",
                stream.endpoint(),
                e
            );
        } else {
            SENT_STRUCT_LOG_COUNT.inc();
            SENT_STRUCT_LOG_BYTES.inc_by(message_length as u64);
        }
    }
}

/// A trait encapsulating the operations required for writing logs.
pub trait Writer: Send + Sync {
    /// Write the log.
    fn write(&self, log: String);

    /// Write the log in an async task.
    fn write_buferred(&mut self, log: String);
}

/// A struct for writing logs to stdout
struct StdoutWriter {
    buffer: std::io::BufWriter<Stdout>,
}

impl StdoutWriter {
    pub fn new() -> Self {
        let buffer = std::io::BufWriter::new(std::io::stdout());
        Self { buffer }
    }
}
impl Writer for StdoutWriter {
    /// Write log to stdout
    fn write(&self, log: String) {
        println!("{}", log);
    }
    fn write_buferred(&mut self, log: String) {
        self.buffer
            .write_fmt(format_args!("{}\n", log))
            .unwrap_or_default();
    }
}

/// A struct for writing logs to a file
pub struct FileWriter {
    log_file: RwLock<std::fs::File>,
}

impl FileWriter {
    pub fn new(log_file: std::path::PathBuf) -> Self {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_file)
            .expect("Unable to open log file");
        Self {
            log_file: RwLock::new(file),
        }
    }
}

impl Writer for FileWriter {
    /// Write to file
    fn write(&self, log: String) {
        if let Err(err) = writeln!(self.log_file.write(), "{}", log) {
            eprintln!("Unable to write to log file: {}", err);
        }
    }
    fn write_buferred(&mut self, log: String) {
        self.write(log);
    }
}

/// Converts a record into a string representation:
/// UNIX_TIMESTAMP LOG_LEVEL [thread_name] FILE:LINE MESSAGE JSON_DATA
/// Example:
/// 2020-03-07 05:03:03 INFO [thread_name] common/aptos-logger/src/lib.rs:261 Hello { "world": true }
fn text_format(entry: &LogEntry) -> Result<String, fmt::Error> {
    use std::fmt::Write;

    let mut w = String::new();
    write!(w, "{}", entry.timestamp)?;

    if let Some(thread_name) = &entry.thread_name {
        write!(w, " [{}]", thread_name)?;
    }

    write!(
        w,
        " {} {}",
        entry.metadata.level(),
        entry.metadata.source_path()
    )?;

    if let Some(message) = &entry.message {
        write!(w, " {}", message)?;
    }

    if !entry.data.is_empty() {
        write!(w, " {}", serde_json::to_string(&entry.data).unwrap())?;
    }

    Ok(w)
}

// converts a record into json format
fn json_format(entry: &LogEntry) -> Result<String, fmt::Error> {
    match serde_json::to_string(&entry) {
        Ok(s) => Ok(s),
        Err(_) => {
            // TODO: Improve the error handling here. Currently we're just increasing some misleadingly-named metric and dropping any context on why this could not be deserialized.
            STRUCT_LOG_PARSE_ERROR_COUNT.inc();
            Err(fmt::Error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LogEntry;
    use crate::{
        aptos_logger::json_format, debug, error, info, logger::Logger, trace, warn, Event, Key,
        KeyValue, Level, Metadata, Schema, Value, Visitor,
    };
    use chrono::{DateTime, Utc};
    #[cfg(test)]
    use pretty_assertions::assert_eq;
    use serde_json::Value as JsonValue;
    use std::{
        sync::{
            mpsc::{self, Receiver, SyncSender},
            Arc,
        },
        thread,
    };

    #[derive(serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Enum {
        FooBar,
    }

    struct TestSchema<'a> {
        foo: usize,
        bar: &'a Enum,
    }

    impl Schema for TestSchema<'_> {
        fn visit(&self, visitor: &mut dyn Visitor) {
            visitor.visit_pair(Key::new("foo"), Value::from_serde(&self.foo));
            visitor.visit_pair(Key::new("bar"), Value::from_serde(&self.bar));
        }
    }

    struct LogStream {
        sender: SyncSender<LogEntry>,
        enable_backtrace: bool,
    }

    impl LogStream {
        fn new(enable_backtrace: bool) -> (Self, Receiver<LogEntry>) {
            let (sender, receiver) = mpsc::sync_channel(1024);
            let log_stream = Self {
                sender,
                enable_backtrace,
            };
            (log_stream, receiver)
        }
    }

    impl Logger for LogStream {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= Level::Debug
        }

        fn record(&self, event: &Event) {
            let entry = LogEntry::new(
                event,
                ::std::thread::current().name(),
                self.enable_backtrace,
            );
            self.sender.send(entry).unwrap();
        }

        fn flush(&self) {}
    }

    fn set_test_logger() -> Receiver<LogEntry> {
        let (logger, receiver) = LogStream::new(true);
        let logger = Arc::new(logger);
        crate::logger::set_global_logger(logger, None);
        receiver
    }

    // TODO: Find a better mechanism for testing that allows setting the logger not globally
    #[test]
    fn basic() {
        let receiver = set_test_logger();
        let number = 12345;

        // Send an info log
        let before = Utc::now();
        let mut line_num = line!();
        info!(
            TestSchema {
                foo: 5,
                bar: &Enum::FooBar
            },
            test = true,
            category = "name",
            KeyValue::new("display", Value::from_display(&number)),
            "This is a log"
        );

        let after = Utc::now();

        let mut entry = receiver.recv().unwrap();

        // Ensure standard fields are filled
        assert_eq!(entry.metadata.level(), Level::Info);
        assert_eq!(
            entry.metadata.target(),
            module_path!().split("::").next().unwrap()
        );
        assert_eq!(entry.message.as_deref(), Some("This is a log"));
        assert!(entry.backtrace.is_none());

        // Ensure json formatter works
        // hardcoding a timestamp and hostname to make the tests deterministic and not depend on environment
        let original_timestamp = entry.timestamp;
        entry.timestamp = String::from("2022-07-24T23:42:29.540278Z");
        entry.hostname = Some("test-host");
        line_num += 1;
        let thread_name = thread::current().name().map(|s| s.to_string()).unwrap();

        let expected = format!("{{\"level\":\"INFO\",\"source\":{{\"package\":\"aptos_logger\",\"file\":\"crates/aptos-logger/src/aptos_logger.rs:{line_num}\"}},\"thread_name\":\"{thread_name}\",\"hostname\":\"test-host\",\"timestamp\":\"2022-07-24T23:42:29.540278Z\",\"message\":\"This is a log\",\"data\":{{\"bar\":\"foo_bar\",\"category\":\"name\",\"display\":\"12345\",\"foo\":5,\"test\":true}}}}");

        assert_eq!(json_format(&entry).unwrap(), expected);

        entry.timestamp = original_timestamp;

        // Log time should be the time the structured log entry was created
        let timestamp = DateTime::parse_from_rfc3339(&entry.timestamp).unwrap();
        let timestamp: DateTime<Utc> = DateTime::from(timestamp);
        assert!(before <= timestamp && timestamp <= after);

        // Ensure data stored is the right type
        assert_eq!(
            entry.data.get(&Key::new("foo")).and_then(JsonValue::as_u64),
            Some(5)
        );
        assert_eq!(
            entry.data.get(&Key::new("bar")).and_then(JsonValue::as_str),
            Some("foo_bar")
        );
        assert_eq!(
            entry
                .data
                .get(&Key::new("display"))
                .and_then(JsonValue::as_str),
            Some(format!("{}", number)).as_deref(),
        );
        assert_eq!(
            entry
                .data
                .get(&Key::new("test"))
                .and_then(JsonValue::as_bool),
            Some(true),
        );
        assert_eq!(
            entry
                .data
                .get(&Key::new("category"))
                .and_then(JsonValue::as_str),
            Some("name"),
        );

        // Test error logs contain backtraces
        error!("This is an error log");
        let entry = receiver.recv().unwrap();
        assert!(entry.backtrace.is_some());

        // Test all log levels work properly
        // Tracing should be skipped because the Logger was setup to skip Tracing events
        trace!("trace");
        debug!("debug");
        info!("info");
        warn!("warn");
        error!("error");

        let levels = &[Level::Debug, Level::Info, Level::Warn, Level::Error];

        for level in levels {
            let entry = receiver.recv().unwrap();
            assert_eq!(entry.metadata.level(), *level);
        }

        // Verify that the thread name is properly included
        let handler = thread::Builder::new()
            .name("named thread".into())
            .spawn(|| info!("thread"))
            .unwrap();

        handler.join().unwrap();
        let entry = receiver.recv().unwrap();
        assert_eq!(entry.thread_name.as_deref(), Some("named thread"));

        // Test Debug and Display inputs
        let debug_struct = DebugStruct {};
        let display_struct = DisplayStruct {};

        error!(identifier = ?debug_struct, "Debug test");
        error!(identifier = ?debug_struct, other = "value", "Debug2 test");
        error!(identifier = %display_struct, "Display test");
        error!(identifier = %display_struct, other = "value", "Display2 test");
        error!("Literal" = ?debug_struct, "Debug test");
        error!("Literal" = ?debug_struct, other = "value", "Debug test");
        error!("Literal" = %display_struct, "Display test");
        error!("Literal" = %display_struct, other = "value", "Display2 test");
        error!("Literal" = %display_struct, other = "value", identifier = ?debug_struct, "Mixed test");
    }

    struct DebugStruct {}

    impl std::fmt::Debug for DebugStruct {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "DebugStruct!")
        }
    }

    struct DisplayStruct {}

    impl std::fmt::Display for DisplayStruct {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "DisplayStruct!")
        }
    }
}
