//! The TOPPING models toppingctl knows, by USB product id.
//!
//! Only the E2x2 is supported. The E4x4 Pre and E8x8 Pre are recognised so
//! toppingctl can say what it found, but they are experimental: they use a
//! different register map (#74) and nothing is ever sent to them yet. Their
//! ids, counts and the E4x4 firmware floor come from our static analysis of
//! the official app (epic #70); none is verified on hardware.

/// USB vendor id of every TOPPING interface here.
pub const VENDOR_ID: u16 = 0x152a;

/// The oldest E4x4 Pre firmware (bcdDevice) the official app accepts: 1.06.
pub const E4X4_MIN_FIRMWARE: u16 = 0x0106;

/// A TOPPING USB audio interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    E2x2,
    E4x4Pre,
    E8x8Pre,
}

/// How many of each a model has. Mixes, outputs and loopbacks are stereo
/// pairs (OUT 1+2, LOOP 1+2, …); inputs are mono channels. The E8x8 Pre's
/// S/PDIF and ADAT outputs are not counted in `outputs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub inputs: usize,
    pub mixes: usize,
    pub outputs: usize,
    pub loopbacks: usize,
    /// Headphone outputs (on the new map: the output groups that carry a
    /// Phone +17 dBu register; unverified).
    pub phones: usize,
}

impl Model {
    /// Every model, E2x2 first: when several are attached, the first found
    /// in this order is used.
    pub const ALL: [Model; 3] = [Model::E2x2, Model::E4x4Pre, Model::E8x8Pre];

    /// The model with USB id `vendor:product`, if any.
    pub fn from_usb_id(vendor: u16, product: u16) -> Option<Model> {
        (vendor == VENDOR_ID)
            .then(|| Self::ALL.into_iter().find(|m| m.product_id() == product))
            .flatten()
    }

    /// The USB product id.
    pub fn product_id(self) -> u16 {
        match self {
            Model::E2x2 => 0x8752,
            Model::E4x4Pre => 0x8754,
            Model::E8x8Pre => 0x875b,
        }
    }

    /// The display name, without "TOPPING".
    pub fn name(self) -> &'static str {
        match self {
            Model::E2x2 => "E2x2",
            Model::E4x4Pre => "E4x4 Pre",
            Model::E8x8Pre => "E8x8 Pre",
        }
    }

    /// Whether the model is only opened with `--experimental`.
    pub fn is_experimental(self) -> bool {
        self != Model::E2x2
    }

    /// The channel counts.
    pub fn counts(self) -> Counts {
        match self {
            Model::E2x2 => Counts {
                inputs: 2,
                mixes: 3,
                outputs: 1,
                loopbacks: 2,
                phones: 1,
            },
            Model::E4x4Pre => Counts {
                inputs: 4,
                mixes: 4,
                outputs: 2,
                loopbacks: 3,
                phones: 2,
            },
            Model::E8x8Pre => Counts {
                inputs: 8,
                mixes: 4,
                outputs: 4,
                loopbacks: 3,
                phones: 2,
            },
        }
    }

    /// The oldest firmware (bcdDevice) toppingctl accepts, if it checks.
    pub fn min_firmware(self) -> Option<u16> {
        match self {
            Model::E4x4Pre => Some(E4X4_MIN_FIRMWARE),
            Model::E2x2 | Model::E8x8Pre => None,
        }
    }

    /// A bcdDevice as a version: `0x0106` is "1.06".
    pub fn firmware_text(bcd: u16) -> String {
        format!("{:x}.{:02x}", bcd >> 8, bcd & 0xff)
    }
}
