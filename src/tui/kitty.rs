//! The kitty graphics protocol's plumbing (#40): asking the terminal
//! whether it takes images and how big its cells are, and the escapes that
//! send an image. It replaces ratatui-image (terminal detection), flate2
//! and base64-simd.
//!
//! - [`detect`] writes [`QUERY`] (a 1×1 graphics query, the cell size
//!   `CSI 16 t`, then primary device attributes, which every terminal
//!   answers) and reads the replies until the attributes come back.
//!   Without a cell size in the reply it falls back to the window size
//!   (TIOCGWINSZ, through crossterm). WezTerm and Konsole are skipped:
//!   their kitty support is partial.
//! - [`write_transmit`] sends uncompressed RGBA (`f=32`) in base64 chunks
//!   of at most 4096 bytes. Images are sent once and cached, so
//!   compression bought little.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

/// The support query: a 1×1 RGB image query under id 31, the cell size
/// in pixels, and primary device attributes to end on.
pub const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";

/// How long to wait for the terminal's next reply before giving up.
const REPLY_TIMEOUT: Duration = Duration::from_millis(2000);

/// The most base64 bytes in one kitty escape (the protocol's limit).
pub const CHUNK: usize = 4096;

/// What the terminal answered to [`QUERY`] so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// It answered the graphics query with `OK`.
    pub kitty: bool,
    /// Its cell size in pixels (width, height), when it gave one.
    pub cell: Option<(u16, u16)>,
    /// The device attributes came back: nothing more is coming.
    pub done: bool,
}

/// Reads the terminal's replies to [`QUERY`] out of `bytes`, skipping
/// anything else. An unfinished sequence at the end is left for later.
pub fn parse_reply(bytes: &[u8]) -> Reply {
    let mut reply = Reply::default();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            // APC: a graphics reply, up to ST.
            Some(b'_') => {
                let body = &bytes[i + 2..];
                let Some(end) = body.windows(2).position(|w| w == b"\x1b\\") else {
                    break;
                };
                if &body[..end] == b"Gi=31;OK" {
                    reply.kitty = true;
                }
                i += 2 + end + 2;
            }
            // CSI: parameter bytes, then a final byte.
            Some(b'[') => {
                let body = &bytes[i + 2..];
                let Some(fin) = body.iter().position(|b| (0x40..=0x7e).contains(b)) else {
                    break;
                };
                let params = &body[..fin];
                match body[fin] {
                    b'c' if params.first() == Some(&b'?') => reply.done = true,
                    b't' => {
                        if let Some(cell) = cell_reply(params) {
                            reply.cell = Some(cell);
                        }
                    }
                    _ => {}
                }
                i += 2 + fin + 1;
            }
            _ => i += 1,
        }
    }
    reply
}

/// `6;height;width` (the reply to `CSI 16 t`) as (width, height).
fn cell_reply(params: &[u8]) -> Option<(u16, u16)> {
    let s = std::str::from_utf8(params).ok()?;
    let mut it = s.split(';');
    if it.next()? != "6" {
        return None;
    }
    let h: u16 = it.next()?.parse().ok()?;
    let w: u16 = it.next()?.parse().ok()?;
    (it.next().is_none() && w > 0 && h > 0).then_some((w, h))
}

/// The cell size in pixels from a window size (TIOCGWINSZ): its pixel
/// size over its cells. `None` when the terminal reports no pixel size.
pub fn cell_size(columns: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 {
        return None;
    }
    let (w, h) = (width / columns, height / rows);
    (w > 0 && h > 0).then_some((w, h))
}

/// The terminal's cell size in pixels from its window size (TIOCGWINSZ),
/// when it reports one.
pub fn window_cell_size() -> Option<(u16, u16)> {
    let w = crossterm::terminal::window_size().ok()?;
    cell_size(w.columns, w.rows, w.width, w.height)
}

/// Whether the terminal named by `WEZTERM_EXECUTABLE` / `KONSOLE_VERSION`
/// (their values) should not be asked: both claim kitty graphics but
/// implement only part of it.
pub fn partial_kitty(wezterm: Option<&OsStr>, konsole: Option<&OsStr>) -> bool {
    wezterm.is_some_and(|v| !v.is_empty()) || konsole.is_some_and(|v| !v.is_empty())
}

/// Asks the terminal (in raw mode, before any event is read) whether it
/// takes kitty images; its cell size in pixels when it does.
pub fn detect() -> Option<(u16, u16)> {
    if partial_kitty(
        std::env::var_os("WEZTERM_EXECUTABLE").as_deref(),
        std::env::var_os("KONSOLE_VERSION").as_deref(),
    ) {
        return None;
    }
    let mut out = io::stdout();
    out.write_all(QUERY.as_bytes()).ok()?;
    out.flush().ok()?;
    // Read on a thread so a terminal that never answers can't hang us;
    // the thread stops reading as soon as the attributes come back.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut got = Vec::new();
        // At least stdin's buffer size, so reads skip its buffer and no
        // key typed after the replies is held there.
        let mut buf = [0u8; 8192];
        let mut stdin = io::stdin();
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
            let reply = parse_reply(&got);
            if tx.send(reply).is_err() || reply.done {
                return;
            }
        }
    });
    let reply = loop {
        let reply = rx.recv_timeout(REPLY_TIMEOUT).ok()?;
        if reply.done {
            break reply;
        }
    };
    if !reply.kitty {
        return None;
    }
    reply.cell.or_else(window_cell_size)
}

/// Standard base64 (RFC 4648), padded.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        for k in 0..4 {
            if k <= c.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * k)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Writes the escapes that send `rgba` (`width`×`height`, 4 bytes a
/// pixel, uncompressed) to the terminal under image id `id`, quietly, in
/// [`CHUNK`]-byte pieces. Returns the base64 bytes written.
pub fn write_transmit(out: &mut String, id: u32, width: u32, height: u32, rgba: &[u8]) -> usize {
    let b64 = base64(rgba);
    // Base64 is ASCII, so any byte boundary is a char boundary.
    let n = b64.len().div_ceil(CHUNK).max(1);
    for i in 0..n {
        let chunk = b64
            .get(i * CHUNK..((i + 1) * CHUNK).min(b64.len()))
            .unwrap_or("");
        let m = u8::from(i + 1 < n);
        if i == 0 {
            let _ = write!(
                out,
                "\x1b_Ga=t,i={id},f=32,s={width},v={height},q=2,m={m};{chunk}\x1b\\"
            );
        } else {
            let _ = write!(out, "\x1b_Gm={m};{chunk}\x1b\\");
        }
    }
    b64.len()
}
