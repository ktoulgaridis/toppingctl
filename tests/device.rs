//! Unit tests for `toppingctl::device`.

use toppingctl::device::*;

fn e2x2(interface_number: i32) -> HidEntry {
    HidEntry {
        vendor_id: VENDOR_ID,
        product_id: PRODUCT_ID,
        interface_number,
    }
}

fn other(vendor_id: u16, product_id: u16, interface_number: i32) -> HidEntry {
    HidEntry {
        vendor_id,
        product_id,
        interface_number,
    }
}

#[test]
fn empty_list_is_not_found() {
    assert_eq!(select(&[]), Err(SelectError::NotFound));
}

#[test]
fn picks_interface_four_among_unrelated_devices() {
    let list = [
        other(0x046d, 0xc52b, 2),
        other(VENDOR_ID, 0x0001, 4),
        other(0x0001, PRODUCT_ID, 4),
        e2x2(4),
    ];
    assert_eq!(select(&list), Ok(3));
}

#[test]
fn prefers_interface_four_over_other_e2x2_interfaces() {
    let list = [e2x2(0), e2x2(5), e2x2(4), e2x2(INTERFACE_UNKNOWN)];
    assert_eq!(select(&list), Ok(2));
}

#[test]
fn takes_first_interface_four_entry_when_listed_twice() {
    // macOS lists one entry per top-level collection of the same interface.
    let list = [other(0x046d, 0xc52b, 0), e2x2(4), e2x2(4)];
    assert_eq!(select(&list), Ok(1));
}

#[test]
fn falls_back_to_only_entry_without_interface_number() {
    let list = [
        other(0x046d, 0xc52b, INTERFACE_UNKNOWN),
        e2x2(INTERFACE_UNKNOWN),
    ];
    assert_eq!(select(&list), Ok(1));
}

#[test]
fn several_entries_without_interface_numbers_are_ambiguous() {
    let list = [e2x2(INTERFACE_UNKNOWN), e2x2(INTERFACE_UNKNOWN)];
    assert_eq!(select(&list), Err(SelectError::Ambiguous { matches: 2 }));
}

#[test]
fn only_entry_on_a_known_non_hid_interface_is_not_found() {
    assert_eq!(select(&[e2x2(2)]), Err(SelectError::NotFound));
}

#[test]
fn unrelated_devices_only_is_not_found() {
    let list = [
        other(0x046d, 0xc52b, INTERFACE_UNKNOWN),
        other(0x1234, 0x5678, 4),
    ];
    assert_eq!(select(&list), Err(SelectError::NotFound));
}
