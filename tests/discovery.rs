// SPDX-License-Identifier: GPL-3.0-or-later
use nusb::descriptors::ConfigurationDescriptor;
use uareu::usb::{capture_interface, supported_id};

#[test]
fn only_the_targeted_digitalpersona_family_can_be_opened() {
    assert!(supported_id(0x05ba, 0x000a));
    assert!(!supported_id(0x05ba, 0x0007));
    assert!(!supported_id(0x045e, 0x00ca));
    assert!(!supported_id(0, 0));
}

fn configuration() -> Vec<u8> {
    vec![
        9, 2, 32, 0, 1, 1, 0, 0x80, 50, // configuration
        9, 4, 2, 0, 2, 0xff, 0xff, 0xff, 0, // interface 2, alt 0
        7, 5, 0x81, 3, 64, 0, 1, // interrupt IN
        7, 5, 0x82, 2, 64, 0, 0,
    ] // bulk IN
}

#[test]
fn discovers_the_interface_from_descriptors_instead_of_hardcoding_zero() {
    let raw = configuration();
    let config = ConfigurationDescriptor::new(&raw).unwrap();
    assert_eq!(capture_interface(config).unwrap(), (2, 0));
}

#[test]
fn rejects_wrong_class_or_endpoint_types() {
    for offset in [14, 15, 16, 20, 21, 27, 28] {
        let mut raw = configuration();
        raw[offset] = 0;
        assert!(capture_interface(ConfigurationDescriptor::new(&raw).unwrap()).is_err());
    }
}
