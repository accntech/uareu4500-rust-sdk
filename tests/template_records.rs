use uareu::template_io::{self, FingerprintView, Minutia, MultiViewRecord, TemplateFormat};

fn record() -> MultiViewRecord {
    MultiViewRecord {
        width: 384,
        height: 290,
        pixels_per_cm_x: 197,
        pixels_per_cm_y: 197,
        vendor_id: 0x103,
        subformat: 0,
        device_info: 0,
        views: vec![view(2, 0), view(7, 0), view(2, 1)],
    }
}
fn view(finger_position: u8, view_number: u8) -> FingerprintView {
    FingerprintView {
        finger_position,
        view_number,
        impression: 0,
        quality: 80,
        minutiae: vec![Minutia {
            x: 10,
            y: 20,
            kind: 1,
            direction_clockwise: 0.0,
            quality: 90,
        }],
    }
}
const FORMATS: [TemplateFormat; 2] = [
    TemplateFormat::Ansi378_2004,
    TemplateFormat::Iso19794_2_2005,
];

#[test]
fn multiview_wire_fixture_and_legacy_boundary() {
    for format in FORMATS {
        let bytes = template_io::encode_views(&record(), format).unwrap();
        let header = if format == TemplateFormat::Ansi378_2004 {
            26
        } else {
            24
        };
        assert_eq!(
            &bytes[header..header + 12],
            &[2, 0, 80, 1, 0x40, 10, 0, 20, 0, 90, 0, 0]
        );
        assert_eq!(bytes[header + 25], 0x10);
        let decoded = template_io::decode_views(&bytes, format).unwrap();
        assert_eq!(decoded.views.len(), 3);
        assert_eq!(decoded.views[2].view_number, 1);
        assert_eq!(template_io::encode_views(&decoded, format).unwrap(), bytes);
        assert!(template_io::decode(&bytes, format).is_err());
        for end in 0..bytes.len() {
            assert!(template_io::decode_views(&bytes[..end], format).is_err());
        }
        let mut reserved = bytes.clone();
        reserved[header - 1] = 1;
        assert!(template_io::decode_views(&reserved, format).is_err());
        let mut extension = bytes.clone();
        extension[header + 11] = 1;
        assert!(template_io::decode_views(&extension, format).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(template_io::decode_views(&trailing, format).is_err());
    }
}
#[test]
fn edits_are_validated_and_atomic() {
    let mut data = record();
    data.edit_view(2, FORMATS[0], |v| {
        v.quality = 55;
        v.minutiae[0].x = 70;
    })
    .unwrap();
    assert_eq!(data.views[2].quality, 55);
    assert!(
        data.edit_view(2, FORMATS[0], |v| v.view_number = 0)
            .is_err()
    );
    assert_eq!(data.views[2].view_number, 1);
    assert!(data.edit_view(10, FORMATS[0], |_| {}).is_err());
    assert!(data.edit(FORMATS[0], |r| r.width = 1).is_err());
    assert_eq!(data.width, 384);
    data.edit(FORMATS[0], |r| r.views.clear()).unwrap();
    let bytes = template_io::encode_views(&data, FORMATS[0]).unwrap();
    assert_eq!(bytes.len(), 26);
    assert!(
        template_io::decode_views(&bytes, FORMATS[0])
            .unwrap()
            .views
            .is_empty()
    );
}
#[test]
fn rejects_invalid_view_metadata_and_supports_long_ansi_length() {
    for format in FORMATS {
        for (index, number) in [(0, 1), (2, 0), (2, 2), (2, 16)] {
            let mut data = record();
            data.views[index].view_number = number;
            assert!(template_io::encode_views(&data, format).is_err());
        }
    }
    let mut data = record();
    data.views[0].impression = 9;
    assert!(template_io::encode_views(&data, FORMATS[0]).is_ok());
    assert!(template_io::encode_views(&data, FORMATS[1]).is_err());
    data = record();
    data.pixels_per_cm_x = 98;
    assert!(template_io::encode_views(&data, FORMATS[1]).is_err());
    data = record();
    data.device_info = 0x1000;
    assert!(template_io::encode_views(&data, FORMATS[0]).is_err());
    data = record();
    data.views.clear();
    for finger in 0..11 {
        for number in 0..16 {
            let mut v = view(finger, number);
            v.minutiae = vec![v.minutiae[0].clone(); 255];
            data.views.push(v);
        }
    }
    let bytes = template_io::encode_views(&data, FORMATS[0]).unwrap();
    assert!(bytes.len() > 65535);
    assert_eq!(&bytes[8..10], &[0, 0]);
    assert_eq!(
        template_io::decode_views(&bytes, FORMATS[0])
            .unwrap()
            .views
            .len(),
        176
    );
}

#[test]
fn independent_empty_envelopes_and_single_view_compatibility() {
    // Hand-assembled from the documented field order, not produced by encoder.
    let iso = [
        b'F', b'M', b'R', 0, b' ', b'2', b'0', 0, 0, 0, 0, 24, 0, 0, 1, 128, 1, 34, 0, 197, 0, 197,
        0, 0,
    ];
    let ansi = [
        b'F', b'M', b'R', 0, b' ', b'2', b'0', 0, 0, 26, 1, 3, 0, 0, 0, 0, 1, 128, 1, 34, 0, 197,
        0, 197, 0, 0,
    ];
    for (bytes, format) in [(&iso[..], FORMATS[1]), (&ansi[..], FORMATS[0])] {
        let data = template_io::decode_views(bytes, format).unwrap();
        assert!(data.views.is_empty());
        assert_eq!(data.width, 384);
        assert_eq!(template_io::encode_views(&data, format).unwrap(), bytes);
        assert!(template_io::decode(bytes, format).is_err());
    }
    let data = record();
    let single = template_io::MinutiaeRecord {
        width: data.width,
        height: data.height,
        pixels_per_cm_x: data.pixels_per_cm_x,
        pixels_per_cm_y: data.pixels_per_cm_y,
        finger_position: 2,
        impression: 0,
        quality: 80,
        vendor_id: data.vendor_id,
        subformat: data.subformat,
        device_info: data.device_info,
        minutiae: data.views[0].minutiae.clone(),
    };
    for format in FORMATS {
        let bytes = template_io::encode(&single, format).unwrap();
        assert_eq!(
            template_io::encode_views(&single.clone().into(), format).unwrap(),
            bytes
        );
        let decoded = template_io::decode_views(&bytes, format).unwrap();
        assert_eq!(decoded.views.len(), 1);
        assert_eq!(decoded.views[0].minutiae[0].x, single.minutiae[0].x);
    }
}

#[test]
fn malformed_view_payloads_are_rejected() {
    for format in FORMATS {
        let bytes = template_io::encode_views(&record(), format).unwrap();
        let header = if format == FORMATS[0] { 26 } else { 24 };
        // Position, impression, view quality, minutia kind, reserved Y, minutia quality,
        // incorrect view count, and a duplicate view number in the third view.
        for (offset, value) in [
            (header, 11),
            (header + 1, 4),
            (header + 2, 101),
            (header + 4, 0xc0),
            (header + 6, 0x40),
            (header + 9, 101),
            (header - 2, 2),
            (header + 25, 0),
        ] {
            let mut corrupt = bytes.clone();
            corrupt[offset] = value;
            assert!(
                template_io::decode_views(&corrupt, format).is_err(),
                "offset {offset}"
            );
        }
        if format == FORMATS[0] {
            let mut corrupt = bytes;
            corrupt[header + 8] = 180;
            assert!(template_io::decode_views(&corrupt, format).is_err());
        }
    }
}
