//! The kitty graphics plumbing that replaced ratatui-image, image, flate2
//! and base64-simd (#40): base64, zlib, the transmit escape, the support
//! query's reply and the cell-size probe.

use toppingctl::tui::kitty::{
    adler32, base64, cell_size, parse_reply, write_transmit, zlib, Reply, QUERY,
};

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

/// A small image goes in one escape: RGBA (`f=32`), zlib-compressed
/// (`o=z`), its size, quiet, no continuation.
#[test]
fn transmit_sends_zlib_rgba() {
    let rgba = [255, 0, 0, 255, 0, 255, 0, 128];
    let mut out = String::new();
    let sent = write_transmit(&mut out, 7, 2, 1, &rgba);
    let b64 = base64(&zlib(&rgba));
    assert_eq!(sent, b64.len());
    assert_eq!(
        out,
        format!("\x1b_Ga=t,i=7,f=32,o=z,s=2,v=1,q=2,m=0;{b64}\x1b\\")
    );
}

/// Bytes that don't compress (a fixed pseudo-random sequence).
fn noise(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// A big image goes in 4096-byte base64 chunks; every chunk but the last
/// says more follows (`m=1`), and only the first carries the keys.
#[test]
fn transmit_chunks_at_4096() {
    let rgba = noise(40 * 40 * 4);
    let mut out = String::new();
    let sent = write_transmit(&mut out, 3, 40, 40, &rgba);
    let b64 = base64(&zlib(&rgba));
    assert_eq!(sent, b64.len());
    let escapes: Vec<&str> = out.split("\x1b\\").filter(|s| !s.is_empty()).collect();
    assert!(escapes.len() > 2);
    assert_eq!(escapes.len(), b64.len().div_ceil(4096));
    assert!(escapes[0].starts_with("\x1b_Ga=t,i=3,f=32,o=z,s=40,v=40,q=2,m=1;"));
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

/// The Adler-32 vector from its Wikipedia article.
#[test]
fn adler32_known_vector() {
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    assert_eq!(adler32(b""), 1);
}

/// zlib (RFC 1950/1951) round-trips through an independent inflater:
/// empty, short, flat runs, repeated rows, noise, and long enough for the
/// Adler sums to wrap.
#[test]
fn zlib_round_trips() {
    let mut rows = Vec::new();
    for y in 0..30u8 {
        for x in 0..50u8 {
            rows.extend([x, y / 3, 7, if x > 25 { 255 } else { 0 }]);
        }
    }
    let inputs: Vec<Vec<u8>> = vec![
        vec![],
        b"a".to_vec(),
        b"abcabcabcabcabcabc".to_vec(),
        vec![0; 100_000],
        [1u8, 2, 3, 4].repeat(5000),
        rows,
        noise(70_000),
    ];
    for raw in inputs {
        let z = zlib(&raw);
        assert_eq!(&z[..2], &[0x78, 0x01], "zlib header");
        assert_eq!(inflate_zlib(&z), raw, "{} bytes", raw.len());
    }
}

/// A flat widget compresses well: a 120×120 face of one colour with a
/// transparent margin is a small fraction of its raw size.
#[test]
fn zlib_compresses_flat_images() {
    let mut img = Vec::new();
    for y in 0..120 {
        for x in 0..120 {
            let inside = (10..110).contains(&x) && (10..110).contains(&y);
            img.extend(if inside { [41, 46, 66, 255] } else { [0; 4] });
        }
    }
    let z = zlib(&img);
    assert!(z.len() * 50 < img.len(), "{} of {}", z.len(), img.len());
}

// ------------------------------------------------- a test-only inflater
//
// Fixed-Huffman and stored blocks (RFC 1951 §3.2.4, §3.2.6), written from
// the RFC, to check the encoder against.

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let b = (self.data[self.pos / 8] >> (self.pos % 8)) & 1;
        self.pos += 1;
        u32::from(b)
    }
    /// `n` bits, least significant first (extra bits, headers).
    fn bits(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, i| v | (self.bit() << i))
    }
    /// One more bit of a Huffman code, most significant first.
    fn code(&mut self, v: u32) -> u32 {
        (v << 1) | self.bit()
    }
    fn lit_len(&mut self) -> u32 {
        let mut v = (0..7).fold(0, |v, _| self.code(v));
        if v <= 23 {
            return 256 + v;
        }
        v = self.code(v);
        match v {
            0x30..=0xbf => v - 0x30,
            0xc0..=0xc7 => 280 + v - 0xc0,
            _ => 144 + self.code(v) - 0x190,
        }
    }
}

const LEN_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate_zlib(z: &[u8]) -> Vec<u8> {
    assert_eq!((u16::from(z[0]) << 8 | u16::from(z[1])) % 31, 0, "FCHECK");
    assert_eq!(z[0] & 0x0f, 8, "deflate");
    let mut b = Bits {
        data: &z[2..z.len() - 4],
        pos: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.bit();
        match b.bits(2) {
            0 => {
                b.pos = b.pos.div_ceil(8) * 8;
                let len = b.bits(16);
                let nlen = b.bits(16);
                assert_eq!(len ^ 0xffff, nlen);
                for _ in 0..len {
                    out.push(b.bits(8) as u8);
                }
            }
            1 => loop {
                let sym = b.lit_len();
                match sym {
                    0..=255 => out.push(sym as u8),
                    256 => break,
                    _ => {
                        let k = (sym - 257) as usize;
                        let len = LEN_BASE[k] + b.bits(LEN_EXTRA[k]);
                        let d = (0..5).fold(0, |v, _| b.code(v)) as usize;
                        let dist = (DIST_BASE[d] + b.bits(DIST_EXTRA[d])) as usize;
                        assert!(dist <= out.len(), "distance too far");
                        for _ in 0..len {
                            out.push(out[out.len() - dist]);
                        }
                    }
                }
            },
            t => panic!("block type {t}"),
        }
        if last == 1 {
            break;
        }
    }
    let tail = &z[z.len() - 4..];
    let sum = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    assert_eq!(sum, adler32(&out), "Adler-32");
    out
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
