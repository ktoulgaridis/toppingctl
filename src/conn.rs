//! Device I/O: a [`Connection`] owns a [`Transport`] on one background thread,
//! writes frames to it and turns what the device reports into [`Event`]s.
//!
//! The thread alternates between writing any queued frames and reading with a
//! short timeout, so the transport is never shared across threads. The CLI
//! and TUI use this instead of touching hidapi directly.

use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use hidapi::HidDevice;

use crate::error::is_interrupted;
use crate::frame::{Crc, DecodeError, Dir, Frame, REPORT_LEN};

pub use std::io::Error as IoError;

/// How long the I/O thread waits for a report before it checks for frames to send.
const READ_TIMEOUT_MS: i32 = 10;
/// Read buffer size; larger than [`REPORT_LEN`] so oversized reports are not split.
const READ_BUFFER_LEN: usize = 64;

/// A byte channel to the device, in units of 16-byte HID reports.
pub trait Transport: Send + 'static {
    /// Writes one report (without a report-ID byte).
    fn write(&mut self, report: &[u8]) -> Result<(), IoError>;
    /// Reads one report into `buf`, waiting at most `ms` milliseconds.
    /// Returns 0 when nothing arrived in time.
    fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> Result<usize, IoError>;
}

/// The E2x2's HID interface, as opened by [`crate::open::open_e2x2`].
pub struct HidTransport {
    device: HidDevice,
}

impl HidTransport {
    pub fn new(device: HidDevice) -> Self {
        Self { device }
    }
}

fn hid_error(err: hidapi::HidError) -> IoError {
    match err {
        hidapi::HidError::IoError { error } => error,
        other => IoError::other(other.to_string()),
    }
}

impl Transport for HidTransport {
    fn write(&mut self, report: &[u8]) -> Result<(), IoError> {
        // No report IDs on this device: hidapi wants a leading 0 byte.
        let mut out = Vec::with_capacity(report.len() + 1);
        out.push(0);
        out.extend_from_slice(report);
        self.device.write(&out).map(|_| ()).map_err(hid_error)
    }

    fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> Result<usize, IoError> {
        match self.device.read_timeout(buf, ms) {
            // A signal (such as Ctrl-C) interrupting the read is not a device error.
            Err(e) if is_interrupted(&e) => Ok(0),
            other => other.map_err(hid_error),
        }
    }
}

/// What the device reported.
#[derive(Debug)]
pub enum Event {
    /// A state change or the echo of a written frame.
    Report(Frame),
    /// A periodic meter frame (idle `00 00` frames are dropped).
    Meter(Frame),
    /// A report that is not a valid device→host frame; `raw` is the report,
    /// truncated or zero-padded to 16 bytes.
    Malformed {
        error: DecodeError,
        raw: [u8; REPORT_LEN],
    },
    /// The transport failed. This is the last event; the I/O thread has ended.
    Disconnected(IoError),
}

/// Why [`Connection::send`] did not queue a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The frame turns 48V phantom power on; use
    /// [`Connection::send_confirmed_phantom`] after the user confirms.
    PhantomPowerNeedsConfirmation,
    /// The I/O thread has ended (see [`Event::Disconnected`]).
    Disconnected,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PhantomPowerNeedsConfirmation => {
                write!(f, "frame turns 48V phantom power on and needs confirmation")
            }
            Self::Disconnected => write!(f, "the TOPPING E2x2 is disconnected"),
        }
    }
}

impl std::error::Error for SendError {}

/// A running connection to the device. Dropping it stops and joins the I/O thread.
pub struct Connection {
    commands: Option<Sender<[u8; REPORT_LEN]>>,
    thread: Option<JoinHandle<()>>,
    crc: Crc,
    seen: Seen,
}

/// When the device last sent a valid frame, idle ones included.
type Seen = Arc<Mutex<Option<Instant>>>;

impl Connection {
    /// Starts the I/O thread on `transport`. Frames are sent with the CRC16
    /// filled in ([`Crc::Fill`]).
    pub fn spawn<T: Transport>(transport: T) -> (Connection, Receiver<Event>) {
        Self::spawn_with_crc(transport, Crc::Fill)
    }

    /// As [`Connection::spawn`], but frames carry `crc` in bytes 11–12.
    pub fn spawn_with_crc<T: Transport>(transport: T, crc: Crc) -> (Connection, Receiver<Event>) {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let seen = Seen::default();
        let io_seen = Arc::clone(&seen);
        let thread = thread::spawn(move || run(transport, command_rx, event_tx, io_seen));
        let conn = Connection {
            commands: Some(commands),
            thread: Some(thread),
            crc,
            seen,
        };
        (conn, events)
    }

    /// When the device last sent a valid frame, including the idle `00 00`
    /// frames it streams (about 250/s) while nothing changes. Those are not
    /// events, but they show the link is alive when there is no signal.
    pub fn last_seen(&self) -> Option<Instant> {
        self.seen.lock().map_or(None, |s| *s)
    }

    /// Queues `frame` for writing. Refuses frames that turn 48V phantom power on.
    pub fn send(&self, frame: Frame) -> Result<(), SendError> {
        if frame.turns_on_phantom_power() {
            return Err(SendError::PhantomPowerNeedsConfirmation);
        }
        self.queue(frame)
    }

    /// Queues `frame` for writing even if it turns 48V phantom power on. Call
    /// only after the user has explicitly confirmed it.
    pub fn send_confirmed_phantom(&self, frame: Frame) -> Result<(), SendError> {
        self.queue(frame)
    }

    fn queue(&self, frame: Frame) -> Result<(), SendError> {
        let commands = self.commands.as_ref().ok_or(SendError::Disconnected)?;
        commands
            .send(frame.encode(self.crc))
            .map_err(|_| SendError::Disconnected)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Hanging up the command channel tells the thread to stop.
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The I/O thread's loop.
fn run<T: Transport>(
    mut transport: T,
    commands: Receiver<[u8; REPORT_LEN]>,
    events: Sender<Event>,
    seen: Seen,
) {
    let error = match pump(&mut transport, &commands, &events, &seen) {
        Some(error) => error,
        None => return,
    };
    // Hang up the command channel before reporting, so `send` already fails
    // once the caller sees `Disconnected`.
    drop(commands);
    let _ = events.send(Event::Disconnected(error));
}

/// Writes and reads until the transport fails (`Some`) or the connection is
/// dropped (`None`). A dropped event receiver is not an error: sends still work.
fn pump<T: Transport>(
    transport: &mut T,
    commands: &Receiver<[u8; REPORT_LEN]>,
    events: &Sender<Event>,
    seen: &Seen,
) -> Option<IoError> {
    let mut buf = [0u8; READ_BUFFER_LEN];
    loop {
        loop {
            match commands.try_recv() {
                Ok(report) => {
                    if let Err(e) = transport.write(&report) {
                        return Some(e);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return None,
            }
        }
        let len = match transport.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => continue,
            Ok(len) => len.min(buf.len()),
            Err(e) => return Some(e),
        };
        let event = classify(&buf[..len]);
        if !matches!(event, Some(Event::Malformed { .. })) {
            if let Ok(mut s) = seen.lock() {
                *s = Some(Instant::now());
            }
        }
        if let Some(event) = event {
            let _ = events.send(event);
        }
    }
}

/// The event for one report, or `None` for an idle frame.
fn classify(data: &[u8]) -> Option<Event> {
    match Frame::decode(data, Dir::FromDevice) {
        Ok(f) if (f.group, f.param) == (0x00, 0x00) => None,
        Ok(f) if f.is_meter() => Some(Event::Meter(f)),
        Ok(f) => Some(Event::Report(f)),
        Err(error) => {
            let mut raw = [0u8; REPORT_LEN];
            let n = data.len().min(REPORT_LEN);
            raw[..n].copy_from_slice(&data[..n]);
            Some(Event::Malformed { error, raw })
        }
    }
}

/// An in-memory [`Transport`] for tests: reads come from an injectable queue
/// and writes go to a shared log.
pub mod fake {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Condvar, Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{IoError, Transport};
    use crate::frame::REPORT_LEN;

    #[derive(Default)]
    struct State {
        reads: VecDeque<Result<Vec<u8>, IoError>>,
        written: Vec<[u8; REPORT_LEN]>,
        fail_writes: Option<io::ErrorKind>,
        dropped: bool,
    }

    #[derive(Default)]
    struct Shared {
        state: Mutex<State>,
        written: Condvar,
    }

    impl Shared {
        fn lock(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// The transport half, handed to [`super::Connection::spawn`].
    pub struct FakeTransport {
        shared: Arc<Shared>,
    }

    /// The test's half: injects reads and inspects writes.
    #[derive(Clone)]
    pub struct FakeHandle {
        shared: Arc<Shared>,
    }

    impl FakeTransport {
        /// A transport with nothing to read, and its handle.
        #[allow(clippy::new_ret_no_self)]
        pub fn new() -> (FakeTransport, FakeHandle) {
            let shared = Arc::new(Shared::default());
            let handle = FakeHandle {
                shared: Arc::clone(&shared),
            };
            (FakeTransport { shared }, handle)
        }
    }

    impl FakeHandle {
        /// Queues a report for the transport to return from a read.
        pub fn push_report(&self, report: &[u8]) {
            self.shared.lock().reads.push_back(Ok(report.to_vec()));
        }

        /// Queues a read error, returned after the reports queued before it.
        pub fn push_read_error(&self, error: IoError) {
            self.shared.lock().reads.push_back(Err(error));
        }

        /// Makes every later write fail with `kind`.
        pub fn fail_writes(&self, kind: io::ErrorKind) {
            self.shared.lock().fail_writes = Some(kind);
        }

        /// The reports written so far, in order.
        pub fn written(&self) -> Vec<[u8; REPORT_LEN]> {
            self.shared.lock().written.clone()
        }

        /// Waits until at least `n` reports were written or `timeout` passed,
        /// and returns the reports written so far.
        pub fn wait_written(&self, n: usize, timeout: Duration) -> Vec<[u8; REPORT_LEN]> {
            let deadline = Instant::now() + timeout;
            let mut state = self.shared.lock();
            while state.written.len() < n {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = match self.shared.written.wait_timeout(state, left) {
                    Ok((s, _)) => s,
                    Err(e) => e.into_inner().0,
                };
            }
            state.written.clone()
        }

        /// Whether the transport has been dropped.
        pub fn is_dropped(&self) -> bool {
            self.shared.lock().dropped
        }
    }

    impl Transport for FakeTransport {
        fn write(&mut self, report: &[u8]) -> Result<(), IoError> {
            let mut state = self.shared.lock();
            if let Some(kind) = state.fail_writes {
                return Err(IoError::new(kind, "fake write failure"));
            }
            let mut out = [0u8; REPORT_LEN];
            let n = report.len().min(REPORT_LEN);
            out[..n].copy_from_slice(&report[..n]);
            state.written.push(out);
            self.shared.written.notify_all();
            Ok(())
        }

        fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> Result<usize, IoError> {
            let next = self.shared.lock().reads.pop_front();
            match next {
                Some(Ok(report)) => {
                    let n = report.len().min(buf.len());
                    buf[..n].copy_from_slice(&report[..n]);
                    Ok(n)
                }
                Some(Err(e)) => Err(e),
                None => {
                    // Like a real read, wait before reporting that nothing arrived.
                    thread::sleep(Duration::from_millis(ms.clamp(0, 10) as u64));
                    Ok(0)
                }
            }
        }
    }

    impl Drop for FakeTransport {
        fn drop(&mut self) {
            self.shared.lock().dropped = true;
        }
    }
}
