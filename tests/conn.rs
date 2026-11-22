use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use toppingctl::conn::fake::FakeTransport;
use toppingctl::conn::{Connection, Event, SendError};
use toppingctl::frame::{Crc, DecodeError, Frame};

const WAIT: Duration = Duration::from_secs(2);

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

fn next(events: &Receiver<Event>) -> Event {
    events.recv_timeout(WAIT).expect("an event within 2 s")
}

#[test]
fn send_writes_encoded_frame() {
    let (transport, fake) = FakeTransport::new();
    let (conn, _events) = Connection::spawn(transport);
    let mon_on = frame(0x21, 0x01, 1);
    let gain = frame(0x22, 0x04, -0x0200_0000);

    conn.send(mon_on).unwrap();
    conn.send(gain).unwrap();

    let written = fake.wait_written(2, WAIT);
    assert_eq!(
        written,
        vec![mon_on.encode(Crc::Fill), gain.encode(Crc::Fill)]
    );
}

#[test]
fn spawn_with_crc_zero_writes_zero_crc() {
    let (transport, fake) = FakeTransport::new();
    let (conn, _events) = Connection::spawn_with_crc(transport, Crc::Zero);
    let mon_on = frame(0x21, 0x01, 1);

    conn.send(mon_on).unwrap();

    assert_eq!(fake.wait_written(1, WAIT), vec![mon_on.encode(Crc::Zero)]);
}

#[test]
fn reports_become_events() {
    let (transport, fake) = FakeTransport::new();
    let state = frame(0x21, 0x01, 1);
    let meter = frame(0x31, 0x03, 0x0123_4567);
    let idle = frame(0x00, 0x00, 0);
    let mut bad_crc = frame(0x22, 0x01, 0).encode(Crc::Fill);
    bad_crc[12] ^= 0xff;
    let echo = frame(0x10, 0x02, 3);
    fake.push_report(&state.encode(Crc::Fill));
    fake.push_report(&meter.encode(Crc::Fill));
    fake.push_report(&idle.encode(Crc::Fill));
    fake.push_report(&bad_crc);
    fake.push_report(&idle.encode(Crc::Fill));
    fake.push_report(&echo.encode(Crc::Fill));

    let (_conn, events) = Connection::spawn(transport);

    assert!(matches!(next(&events), Event::Report(f) if f == state));
    assert!(matches!(next(&events), Event::Meter(f) if f == meter));
    match next(&events) {
        Event::Malformed { error, raw } => {
            assert!(matches!(error, DecodeError::BadCrc { .. }), "{error:?}");
            assert_eq!(raw, bad_crc);
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
    assert!(matches!(next(&events), Event::Report(f) if f == echo));
    assert_eq!(
        events.recv_timeout(Duration::from_millis(100)).err(),
        Some(RecvTimeoutError::Timeout),
        "no events beyond the injected reports"
    );
}

#[test]
fn short_report_is_malformed_with_zero_padding() {
    let (transport, fake) = FakeTransport::new();
    fake.push_report(&[0x22, 0x33, 0x20]);
    let (_conn, events) = Connection::spawn(transport);

    match next(&events) {
        Event::Malformed { error, raw } => {
            assert_eq!(error, DecodeError::TooShort);
            let mut expected = [0u8; 16];
            expected[..3].copy_from_slice(&[0x22, 0x33, 0x20]);
            assert_eq!(raw, expected);
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn phantom_power_needs_confirmation() {
    let (transport, fake) = FakeTransport::new();
    let (conn, _events) = Connection::spawn(transport);
    let in1_48v_on = frame(0x21, 0x02, 1);
    let in2_48v_on = frame(0x22, 0x02, 1);
    let in1_48v_off = frame(0x21, 0x02, 0);
    let marker = frame(0x21, 0x01, 1);

    assert_eq!(
        conn.send(in1_48v_on),
        Err(SendError::PhantomPowerNeedsConfirmation)
    );
    assert_eq!(
        conn.send(in2_48v_on),
        Err(SendError::PhantomPowerNeedsConfirmation)
    );
    // Commands are written in order, so once the marker is written nothing
    // refused before it can still arrive.
    conn.send(marker).unwrap();
    assert_eq!(fake.wait_written(1, WAIT), vec![marker.encode(Crc::Fill)]);

    conn.send_confirmed_phantom(in1_48v_on).unwrap();
    conn.send(in1_48v_off).unwrap();
    assert_eq!(
        fake.wait_written(3, WAIT),
        vec![
            marker.encode(Crc::Fill),
            in1_48v_on.encode(Crc::Fill),
            in1_48v_off.encode(Crc::Fill),
        ]
    );
}

fn assert_disconnects(conn: &Connection, events: &Receiver<Event>, kind: io::ErrorKind) {
    match next(events) {
        Event::Disconnected(e) => assert_eq!(e.kind(), kind),
        other => panic!("expected Disconnected, got {other:?}"),
    }
    // The thread has ended: the event channel hangs up with no further events.
    assert_eq!(
        events.recv_timeout(WAIT).err(),
        Some(RecvTimeoutError::Disconnected)
    );
    assert_eq!(
        conn.send(frame(0x21, 0x01, 1)),
        Err(SendError::Disconnected)
    );
    assert_eq!(
        conn.send_confirmed_phantom(frame(0x21, 0x02, 1)),
        Err(SendError::Disconnected)
    );
}

#[test]
fn io_errors_disconnect() {
    // A read error.
    let (transport, fake) = FakeTransport::new();
    let state = frame(0x21, 0x01, 1);
    fake.push_report(&state.encode(Crc::Fill));
    fake.push_read_error(io::Error::new(io::ErrorKind::BrokenPipe, "unplugged"));
    let (conn, events) = Connection::spawn(transport);
    assert!(matches!(next(&events), Event::Report(f) if f == state));
    assert_disconnects(&conn, &events, io::ErrorKind::BrokenPipe);

    // A write error.
    let (transport, fake) = FakeTransport::new();
    fake.fail_writes(io::ErrorKind::NotConnected);
    let (conn, events) = Connection::spawn(transport);
    conn.send(state).unwrap();
    assert_disconnects(&conn, &events, io::ErrorKind::NotConnected);
    assert!(fake.written().is_empty());
}

#[test]
fn drop_stops_thread() {
    let (transport, fake) = FakeTransport::new();
    let (conn, events) = Connection::spawn(transport);
    conn.send(frame(0x21, 0x01, 1)).unwrap();
    fake.wait_written(1, WAIT);
    assert!(!fake.is_dropped());

    let start = Instant::now();
    drop(conn);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );

    // Joined: the thread has already dropped the transport and the event sender.
    assert!(fake.is_dropped());
    assert_eq!(
        events.try_recv().err(),
        Some(std::sync::mpsc::TryRecvError::Disconnected)
    );
}

/// The device's idle `00 00` frames are not events, but they show the link
/// is alive: `Connection::last_seen` moves with them.
#[test]
fn idle_frames_mark_the_link_alive() {
    let (transport, fake) = FakeTransport::new();
    let (conn, events) = Connection::spawn(transport);
    assert_eq!(conn.last_seen(), None, "nothing has arrived yet");
    let before = std::time::Instant::now();
    fake.push_report(&frame(0x00, 0x00, 0).encode(Crc::Fill));
    let deadline = before + WAIT;
    while conn.last_seen().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let seen = conn.last_seen().expect("an idle frame counts as alive");
    assert!(seen >= before);
    assert!(
        events
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "still no event for an idle frame"
    );
}
