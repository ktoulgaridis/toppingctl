//! Unit tests for `toppingctl::model` and model discovery in
//! `toppingctl::device`: fake enumerations, no hardware.

use toppingctl::device::{
    discover, DeviceEntry, DiscoverError, Found, INTERFACE_UNKNOWN, VENDOR_ID,
};
use toppingctl::model::{Counts, Model, E4X4_MIN_FIRMWARE};

/// A usage page in the vendor-defined range (0xFF00..=0xFFFF).
const VENDOR_PAGE: u16 = 0xff00;
/// Consumer control (volume knob, media keys).
const CONSUMER_PAGE: u16 = 0x000c;
/// What hidapi reports when it could not read the report descriptor.
const PAGE_UNKNOWN: u16 = 0;

fn entry(model: Model, interface_number: i32, usage_page: u16) -> DeviceEntry {
    DeviceEntry {
        vendor_id: VENDOR_ID,
        product_id: model.product_id(),
        interface_number,
        usage_page,
        release_number: 0x0106,
    }
}

fn found(model: Model, index: usize) -> Result<Found, DiscoverError> {
    Ok(Found {
        model,
        index,
        release_number: 0x0106,
    })
}

#[test]
fn usb_ids_map_to_models() {
    assert_eq!(Model::from_usb_id(0x152a, 0x8752), Some(Model::E2x2));
    assert_eq!(Model::from_usb_id(0x152a, 0x8754), Some(Model::E4x4Pre));
    assert_eq!(Model::from_usb_id(0x152a, 0x875b), Some(Model::E8x8Pre));
    assert_eq!(Model::from_usb_id(0x152a, 0x8753), None);
    assert_eq!(Model::from_usb_id(0x046d, 0x8752), None);
    for m in Model::ALL {
        assert_eq!(Model::from_usb_id(VENDOR_ID, m.product_id()), Some(m));
    }
}

#[test]
fn names_and_experimental_flags() {
    assert_eq!(Model::E2x2.name(), "E2x2");
    assert_eq!(Model::E4x4Pre.name(), "E4x4 Pre");
    assert_eq!(Model::E8x8Pre.name(), "E8x8 Pre");
    assert!(!Model::E2x2.is_experimental());
    assert!(Model::E4x4Pre.is_experimental());
    assert!(Model::E8x8Pre.is_experimental());
}

#[test]
fn e2x2_counts_match_the_state() {
    let c = Model::E2x2.counts();
    let s = toppingctl::state::State::default();
    assert_eq!(c.inputs, s.inputs.len());
    assert_eq!(c.mixes, s.mixes.len());
    assert_eq!(c.loopbacks, s.loopbacks.len());
    assert_eq!(
        c,
        Counts {
            inputs: 2,
            mixes: 3,
            outputs: 1,
            loopbacks: 2,
            phones: 1,
        }
    );
}

#[test]
fn new_map_counts_follow_the_spike() {
    assert_eq!(
        Model::E4x4Pre.counts(),
        Counts {
            inputs: 4,
            mixes: 4,
            outputs: 2,
            loopbacks: 3,
            phones: 2,
        }
    );
    assert_eq!(
        Model::E8x8Pre.counts(),
        Counts {
            inputs: 8,
            mixes: 4,
            outputs: 4,
            loopbacks: 3,
            phones: 2,
        }
    );
}

#[test]
fn only_the_e4x4_has_a_firmware_floor() {
    assert_eq!(E4X4_MIN_FIRMWARE, 0x0106);
    assert_eq!(Model::E4x4Pre.min_firmware(), Some(0x0106));
    assert_eq!(Model::E2x2.min_firmware(), None);
    assert_eq!(Model::E8x8Pre.min_firmware(), None);
}

#[test]
fn firmware_versions_read_as_bcd() {
    assert_eq!(Model::firmware_text(0x0106), "1.06");
    assert_eq!(Model::firmware_text(0x0105), "1.05");
    assert_eq!(Model::firmware_text(0x0210), "2.10");
}

#[test]
fn nothing_attached_is_not_found() {
    assert_eq!(discover(&[]), Err(DiscoverError::NotFound));
    let unrelated = DeviceEntry {
        vendor_id: 0x046d,
        product_id: 0xc52b,
        interface_number: 2,
        usage_page: VENDOR_PAGE,
        release_number: 1,
    };
    assert_eq!(discover(&[unrelated]), Err(DiscoverError::NotFound));
}

#[test]
fn e2x2_is_chosen_on_interface_four_as_before() {
    // Usage pages play no part for the E2x2: interface 4 wins.
    let list = [
        entry(Model::E2x2, 0, VENDOR_PAGE),
        entry(Model::E2x2, 4, CONSUMER_PAGE),
    ];
    assert_eq!(discover(&list), found(Model::E2x2, 1));
}

#[test]
fn e2x2_wins_over_an_experimental_model() {
    let list = [
        entry(Model::E4x4Pre, 3, VENDOR_PAGE),
        entry(Model::E2x2, 4, VENDOR_PAGE),
    ];
    assert_eq!(discover(&list), found(Model::E2x2, 1));
}

#[test]
fn e2x2_ambiguity_is_reported_for_the_e2x2() {
    let list = [
        entry(Model::E2x2, INTERFACE_UNKNOWN, VENDOR_PAGE),
        entry(Model::E2x2, INTERFACE_UNKNOWN, VENDOR_PAGE),
        entry(Model::E4x4Pre, 3, VENDOR_PAGE),
    ];
    assert_eq!(
        discover(&list),
        Err(DiscoverError::Ambiguous {
            model: Model::E2x2,
            matches: 2
        })
    );
}

#[test]
fn e4x4_takes_the_vendor_defined_interface_whatever_its_number() {
    for iface in [0, 3, 4, 5] {
        let list = [
            entry(Model::E4x4Pre, 7, CONSUMER_PAGE),
            entry(Model::E4x4Pre, iface, VENDOR_PAGE),
        ];
        assert_eq!(discover(&list), found(Model::E4x4Pre, 1), "iface {iface}");
    }
}

#[test]
fn e8x8_vendor_interface_listed_once_per_collection_takes_the_first() {
    // macOS lists one entry per top-level collection of the same interface.
    let list = [
        entry(Model::E8x8Pre, 2, CONSUMER_PAGE),
        entry(Model::E8x8Pre, 5, VENDOR_PAGE),
        entry(Model::E8x8Pre, 5, 0xff01),
    ];
    assert_eq!(discover(&list), found(Model::E8x8Pre, 1));
}

#[test]
fn two_vendor_defined_interfaces_are_ambiguous() {
    let list = [
        entry(Model::E8x8Pre, 4, VENDOR_PAGE),
        entry(Model::E8x8Pre, 5, VENDOR_PAGE),
    ];
    assert_eq!(
        discover(&list),
        Err(DiscoverError::Ambiguous {
            model: Model::E8x8Pre,
            matches: 2
        })
    );
}

#[test]
fn vendor_entries_without_interface_numbers_count_separately() {
    let one = [entry(Model::E4x4Pre, INTERFACE_UNKNOWN, VENDOR_PAGE)];
    assert_eq!(discover(&one), found(Model::E4x4Pre, 0));
    let two = [
        entry(Model::E4x4Pre, INTERFACE_UNKNOWN, VENDOR_PAGE),
        entry(Model::E4x4Pre, INTERFACE_UNKNOWN, VENDOR_PAGE),
    ];
    assert_eq!(
        discover(&two),
        Err(DiscoverError::Ambiguous {
            model: Model::E4x4Pre,
            matches: 2
        })
    );
}

#[test]
fn unknown_usage_page_falls_back_to_the_only_such_entry() {
    // Linux reports page 0 when the report descriptor can't be read.
    let list = [
        entry(Model::E4x4Pre, 2, CONSUMER_PAGE),
        entry(Model::E4x4Pre, 3, PAGE_UNKNOWN),
    ];
    assert_eq!(discover(&list), found(Model::E4x4Pre, 1));
    let two = [
        entry(Model::E4x4Pre, 2, PAGE_UNKNOWN),
        entry(Model::E4x4Pre, 3, PAGE_UNKNOWN),
    ];
    assert_eq!(
        discover(&two),
        Err(DiscoverError::Ambiguous {
            model: Model::E4x4Pre,
            matches: 2
        })
    );
}

#[test]
fn a_known_non_vendor_page_is_never_chosen() {
    let list = [
        entry(Model::E4x4Pre, 2, CONSUMER_PAGE),
        entry(Model::E4x4Pre, 3, 0x0001),
    ];
    assert_eq!(discover(&list), Err(DiscoverError::NotFound));
}

#[test]
fn e4x4_is_preferred_over_e8x8_when_both_are_attached() {
    let list = [
        entry(Model::E8x8Pre, 5, VENDOR_PAGE),
        entry(Model::E4x4Pre, 3, VENDOR_PAGE),
    ];
    assert_eq!(discover(&list), found(Model::E4x4Pre, 1));
}

#[test]
fn found_carries_the_release_number() {
    let mut e = entry(Model::E4x4Pre, 3, VENDOR_PAGE);
    e.release_number = 0x0105;
    assert_eq!(
        discover(&[e]),
        Ok(Found {
            model: Model::E4x4Pre,
            index: 0,
            release_number: 0x0105
        })
    );
}
