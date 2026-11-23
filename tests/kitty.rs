//! The kitty graphics plumbing that replaced ratatui-image, image, flate2
//! and base64-simd (#40): base64, the transmit escape, the support query's
//! reply and the cell-size probe.

use toppingctl::tui::kitty::{base64, cell_size, parse_reply, write_transmit, Reply, QUERY};

/// RFC 4648 §10's test vectors.
#[test]
fn base64_matches_rfc_4648() {
    for (raw, enc) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(raw.as_bytes()), enc, "{raw:?}");
    }
}

/// Every byte value, and the two characters past the alphanumerics.
#[test]
fn base64_uses_the_standard_alphabet() {
    assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    assert_eq!(base64(&[0, 0, 0]), "AAAA");
    let all: Vec<u8> = (0..=255).collect();
    let enc = base64(&all);
    assert_eq!(enc.len(), 344);
    assert!(enc.starts_with("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g"));
    assert!(enc.ends_with("+/w=="));
}

/// A small image goes in one escape: uncompressed RGBA (`f=32`, no
/// `o=z`), its size, quiet, no continuation.
#[test]
fn transmit_sends_uncompressed_rgba() {
    let rgba = [255, 0, 0, 255, 0, 255, 0, 128];
    let mut out = String::new();
    let sent = write_transmit(&mut out, 7, 2, 1, &rgba);
    let b64 = base64(&rgba);
    assert_eq!(sent, b64.len());
    assert_eq!(
        out,
        format!("\x1b_Ga=t,i=7,f=32,s=2,v=1,q=2,m=0;{b64}\x1b\\")
    );
    assert!(!out.contains("o=z"));
}

/// A big image goes in 4096-byte base64 chunks; every chunk but the last
/// says more follows (`m=1`), and only the first carries the keys.
#[test]
fn transmit_chunks_at_4096() {
    let rgba = vec![9u8; 40 * 40 * 4];
    let mut out = String::new();
    let sent = write_transmit(&mut out, 3, 40, 40, &rgba);
    let b64 = base64(&rgba);
    assert_eq!(sent, b64.len());
    let escapes: Vec<&str> = out.split("\x1b\\").filter(|s| !s.is_empty()).collect();
    assert_eq!(escapes.len(), b64.len().div_ceil(4096));
    assert!(escapes[0].starts_with("\x1b_Ga=t,i=3,f=32,s=40,v=40,q=2,m=1;"));
    for e in &escapes[1..escapes.len() - 1] {
        assert!(e.starts_with("\x1b_Gm=1;"), "{e:?}");
    }
    assert!(escapes.last().unwrap().starts_with("\x1b_Gm=0;"));
    let joined: String = escapes
        .iter()
        .map(|e| e.split_once(';').unwrap().1)
        .collect();
    assert_eq!(joined, b64);
    assert!(escapes
        .iter()
        .all(|e| e.split_once(';').unwrap().1.len() <= 4096));
}

/// The query: a 1×1 kitty query (id 31), the cell size, then primary
/// device attributes, which every terminal answers, to end on.
#[test]
fn query_asks_for_graphics_cell_size_and_attributes() {
    assert_eq!(
        QUERY,
        "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c"
    );
}

#[test]
fn reply_from_kitty() {
    let r = parse_reply(b"\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;22c");
    assert_eq!(
        r,
        Reply {
            kitty: true,
            cell: Some((10, 20)),
            done: true
        }
    );
}

/// A terminal without the protocol answers only the attributes (and maybe
/// the cell size); an error reply is not support either.
#[test]
fn reply_without_kitty() {
    let r = parse_reply(b"\x1b[?64;1;2;4;6;9;15;18;21;22c");
    assert_eq!(
        r,
        Reply {
            kitty: false,
            cell: None,
            done: true
        }
    );
    let r = parse_reply(b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[6;16;8t\x1b[?62c");
    assert!(!r.kitty);
    assert_eq!(r.cell, Some((8, 16)));
    assert!(r.done);
}

/// Replies arrive in pieces: not done until the attributes end; a cell
/// size of 0 is no cell size; stray input around them is skipped.
#[test]
fn reply_in_pieces_and_noise() {
    assert_eq!(parse_reply(b""), Reply::default());
    let partial = parse_reply(b"\x1b_Gi=31;OK\x1b\\\x1b[6;20;1");
    assert!(partial.kitty);
    assert_eq!(partial.cell, None);
    assert!(!partial.done);
    assert!(!parse_reply(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;2").done);
    let r = parse_reply(b"x\x1b[6;0;0t\x1b[Ay\x1b_Gi=31;OK\x1b\\\x1b[?1;2c");
    assert_eq!(
        r,
        Reply {
            kitty: true,
            cell: None,
            done: true
        }
    );
    // The cursor-key form of `c` is not the attributes reply.
    assert!(!parse_reply(b"\x1b[2c").done);
}

/// TIOCGWINSZ: the window's pixel size over its cells; nothing when the
/// terminal leaves the pixel size at 0.
#[test]
fn cell_size_from_window_size() {
    assert_eq!(cell_size(80, 24, 800, 480), Some((10, 20)));
    assert_eq!(cell_size(100, 30, 1234, 789), Some((12, 26)));
    assert_eq!(cell_size(80, 24, 0, 0), None);
    assert_eq!(cell_size(0, 0, 800, 480), None);
    assert_eq!(cell_size(80, 24, 800, 0), None);
    // Fewer pixels than cells: no usable size.
    assert_eq!(cell_size(80, 24, 40, 480), None);
}
