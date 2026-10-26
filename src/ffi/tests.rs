// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::ptr::null_mut;

#[test]
fn ansi381_c_api_roundtrip_raw_view_and_error_ownership() {
    let input=data(&serde_json::json!({"cbeff_product_id":65537,"appendix_f_compliant":false,"record":{
            "capture_device_id":1,"acquisition_level":10,"scale_units":1,"scan_resolution_x":500,"scan_resolution_y":500,
            "image_resolution_x":500,"image_resolution_y":500,"compression":"raw8_bit","images":[{
                "finger_position":1,"view_count":1,"view_number":1,"quality":254,"impression":0,"width":2,"height":2,"data":[0,255,128,64]
            }]}})).unwrap();
    let (mut bytes, mut record, mut image_id) = (99, 99, 99);
    assert_eq!(unsafe { uareu_image_encode_ansi381(input, &mut bytes) }, OK);
    assert_eq!(
        unsafe { uareu_image_decode_ansi381(bytes, &mut record) },
        OK
    );
    assert_eq!(
        unsafe { uareu_image_ansi381_view(record, 0, &mut image_id) },
        OK
    );
    assert_eq!(
        image(image_id).unwrap().captured.image.pixels,
        [0, 255, 128, 64]
    );
    let restored: serde_json::Value = read_data(record).unwrap();
    assert_eq!(restored["record"]["images"][0]["quality"], 254);
    let mut failed = 99;
    assert_eq!(
        unsafe { uareu_image_ansi381_view(record, 1, &mut failed) },
        IMAGE_ERROR
    );
    assert_eq!(failed, 0);
    let bad = data(&vec![0u8, 1, 2]).unwrap();
    failed = 99;
    assert_eq!(
        unsafe { uareu_image_decode_ansi381(bad, &mut failed) },
        IMAGE_ERROR
    );
    assert_eq!(failed, 0);
    for id in [input, bytes, record, bad] {
        assert_eq!(uareu_data_free(id), OK);
    }
    assert_eq!(uareu_image_free(image_id), OK);
}

#[test]
fn async_completion_c_api_validation_and_owned_delivery() {
    use std::sync::mpsc;
    unsafe extern "C" fn callback(
        status: u32,
        image: u64,
        report: u64,
        context: *mut std::ffi::c_void,
    ) {
        let send = unsafe { &*(context as *const mpsc::Sender<(u32, u64, u64)>) };
        let _ = send.send((status, image, report));
    }
    let mut output = 99;
    assert_eq!(
        unsafe {
            uareu_capture_start_with_completion(0, 100, 1, None, std::ptr::null_mut(), &mut output)
        },
        INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
    assert_eq!(
        unsafe {
            uareu_capture_start_with_completion(
                0,
                100,
                1,
                Some(callback),
                std::ptr::null_mut(),
                &mut output,
            )
        },
        INVALID_HANDLE
    );
    assert_eq!(output, 0);
    let (send, receive) = mpsc::channel::<(u32, u64, u64)>();
    let context = &send as *const _ as usize;
    for fails in [false, true] {
        start_callback_operation(
            Arc::new(AtomicBool::new(false)),
            move |_| {
                if fails {
                    Err(CaptureError::new(
                        CaptureErrorKind::Cancelled,
                        CaptureStage::FingerWait,
                        "injected cancellation",
                    )
                    .into())
                } else {
                    Ok(CapturedImage::new(crate::frame::Image {
                        width: 1,
                        height: 1,
                        pixels: vec![17],
                    }))
                }
            },
            callback,
            context,
            &mut output,
        )
        .unwrap();
        let (status, image_id, report) = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        if fails {
            assert_eq!((status, image_id), (CANCELLED, 0));
            let json: serde_json::Value = read_data(report).unwrap();
            assert_eq!(json["kind"], "cancelled");
            assert_eq!(uareu_data_free(report), OK);
        } else {
            assert_eq!((status, report), (OK, 0));
            assert_eq!(image(image_id).unwrap().captured.image.pixels, [17]);
            assert_eq!(uareu_image_free(image_id), OK);
        }
        assert!(task(output).unwrap().status().consumed);
        assert_eq!(uareu_task_free(output), OK);
        assert_eq!(uareu_task_free(output), INVALID_HANDLE);
    }
}

#[test]
fn async_completion_reentrant_free_is_busy_without_removing_task() {
    struct Context {
        job: std::sync::mpsc::Receiver<u64>,
        result: std::sync::mpsc::Sender<(u32, bool, String)>,
    }
    unsafe extern "C" fn callback(
        _status: u32,
        _image: u64,
        report: u64,
        context: *mut std::ffi::c_void,
    ) {
        let context = unsafe { &*(context as *const Context) };
        if let Ok(id) = context.job.recv_timeout(Duration::from_secs(2)) {
            let status = uareu_task_free(id);
            let exists = task(id).is_ok();
            let kind = LAST_ERROR.with(|last| last.borrow().kind.clone());
            let _ = context.result.send((status, exists, kind));
        }
        if report != 0 {
            uareu_data_free(report);
        }
    }
    let (job_send, job_receive) = std::sync::mpsc::channel();
    let (send, receive) = std::sync::mpsc::channel();
    let context = Box::new(Context {
        job: job_receive,
        result: send,
    });
    let mut id = 0;
    start_callback_operation(
        Arc::new(AtomicBool::new(false)),
        |_| Err(anyhow::anyhow!("injected")),
        callback,
        &*context as *const _ as usize,
        &mut id,
    )
    .unwrap();
    job_send.send(id).unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(2)).unwrap(),
        (BUSY, true, "busy".to_owned())
    );
    assert_eq!(uareu_task_free(id), OK);
    drop(context);
}

#[test]
fn legacy_quality_c_api_keeps_scale_unknown_and_rejects_bad_config() {
    let input = register_image(CapturedImage::new(crate::frame::Image {
        width: 2,
        height: 1,
        pixels: vec![0, 255],
    }))
    .unwrap();
    let config = br#"{"engine_path":"missing-nfiq"}"#;
    let mut report = 99;
    assert_eq!(
        unsafe { uareu_image_assess_nfiq1(input, config.as_ptr(), config.len(), &mut report) },
        OK
    );
    let json: serde_json::Value = read_data(report).unwrap();
    assert_eq!(json["status"], "unsupported_input");
    assert_eq!(
        image(input).unwrap().captured.metadata.resolution,
        crate::metadata::Resolution::Unknown
    );
    assert_eq!(uareu_data_free(report), OK);
    for config in [
        br#"{"engine_path":"missing-nfiq","timeout_ms":0}"#.as_slice(),
        br#"{"engine_path":"missing-nfiq","model_path":"model"}"#,
    ] {
        report = 99;
        assert_eq!(
            unsafe { uareu_image_assess_nfiq1(input, config.as_ptr(), config.len(), &mut report) },
            INVALID_ARGUMENT
        );
        assert_eq!(report, 0);
    }
    assert_eq!(uareu_image_free(input), OK);
}

#[test]
fn jpeg2000_c_api_rejects_bad_deadline_and_codestream_without_outputs() {
    let mut output = 99;
    let invalid = br#"{"encoder":"missing","decoder":"missing","timeout_ms":0}"#;
    assert_eq!(
        unsafe { uareu_image_encode_jpeg2000(0, invalid.as_ptr(), invalid.len(), &mut output) },
        INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
    let valid = br#"{"encoder":"missing","decoder":"missing"}"#;
    let bytes = data(&vec![0u8, 1, 2]).unwrap();
    output = 99;
    assert_eq!(
        unsafe { uareu_image_decode_jpeg2000(bytes, valid.as_ptr(), valid.len(), &mut output) },
        IMAGE_ERROR
    );
    assert_eq!(output, 0);
    assert_eq!(uareu_data_free(bytes), OK);
}

#[test]
fn extended_template_c_api_roundtrips_and_clears_failed_outputs() {
    let input = data(&serde_json::json!({
            "record": {"width":20,"height":20,"pixels_per_cm_x":197,"pixels_per_cm_y":197,
                "vendor_id":1,"subformat":0,"device_info":0,
                "views":[{"finger_position":1,"view_number":0,"impression":0,"quality":80,"minutiae":[]}]},
            "extensions":[[{"type_code":257,"data":[0,255,17]}]]
        })).unwrap();
    for format in [0, 1] {
        let (mut bytes, mut restored) = (99, 99);
        assert_eq!(
            unsafe { uareu_template_encode_extended(input, format, &mut bytes) },
            OK
        );
        assert_eq!(
            unsafe { uareu_template_decode_extended(bytes, format, &mut restored) },
            OK
        );
        let json: serde_json::Value = read_data(restored).unwrap();
        assert_eq!(
            json["extensions"][0][0]["data"],
            serde_json::json!([0, 255, 17])
        );
        let mut failed = 99;
        assert_eq!(
            unsafe { uareu_template_decode_views(bytes, format, &mut failed) },
            IMAGE_ERROR
        );
        assert_eq!(failed, 0);
        assert_eq!(
            unsafe { uareu_template_decode_extended(bytes, 99, &mut failed) },
            INVALID_ARGUMENT
        );
        assert_eq!(failed, 0);
        assert_eq!(uareu_data_free(bytes), OK);
        assert_eq!(uareu_data_free(restored), OK);
    }
    assert_eq!(uareu_data_free(input), OK);
    let mut failed = 99;
    assert_eq!(
        unsafe { uareu_template_encode_extended(input, 0, &mut failed) },
        INVALID_HANDLE
    );
    assert_eq!(failed, 0);
}

fn last_error() -> String {
    let mut required = 0;
    assert_eq!(
        unsafe { uareu_last_error(null_mut(), 0, &mut required) },
        BUFFER_TOO_SMALL
    );
    let mut bytes = vec![0; required];
    assert_eq!(
        unsafe { uareu_last_error(bytes.as_mut_ptr(), bytes.len(), &mut required) },
        OK
    );
    String::from_utf8(bytes[..bytes.len() - 1].to_vec()).unwrap()
}

#[test]
fn flat_field_c_api_preserves_inputs_and_rejects_invalid_profiles() {
    let dark = register_image(crate::metadata::CapturedImage::new(crate::frame::Image {
        width: 2,
        height: 1,
        pixels: vec![10, 20],
    }))
    .unwrap();
    let bright = register_image(crate::metadata::CapturedImage::new(crate::frame::Image {
        width: 2,
        height: 1,
        pixels: vec![110, 220],
    }))
    .unwrap();
    let input = register_image(crate::metadata::CapturedImage::new(crate::frame::Image {
        width: 2,
        height: 1,
        pixels: vec![60, 120],
    }))
    .unwrap();
    let (mut profile, mut corrected) = (99, 99);
    assert_eq!(
        unsafe { uareu_image_flat_field_profile(dark, bright, 8.0, &mut profile) },
        OK
    );
    assert_eq!(
        unsafe { uareu_image_apply_flat_field(input, profile, &mut corrected) },
        OK
    );
    assert_eq!(image(input).unwrap().captured.image.pixels, [60, 120]);
    assert_eq!(image(corrected).unwrap().captured.image.pixels, [128, 128]);
    let bad = data(&serde_json::json!({})).unwrap();
    let mut failed = 99;
    assert_ne!(
        unsafe { uareu_image_apply_flat_field(input, bad, &mut failed) },
        OK
    );
    assert_eq!(failed, 0);
    assert_ne!(
        unsafe { uareu_image_flat_field_profile(dark, bright, 0.0, &mut failed) },
        OK
    );
    assert_eq!(failed, 0);
    for id in [profile, bad] {
        assert_eq!(uareu_data_free(id), OK);
    }
    for id in [dark, bright, input, corrected] {
        assert_eq!(uareu_image_free(id), OK);
    }
}
#[test]
fn record_view_assessment_rejects_opaque_without_codec_or_resolution_override() {
    let record = serde_json::json!({"capture_device_id":1,"acquisition_level":0,"scale_units":1,"scan_resolution_x":500,"scan_resolution_y":500,"image_resolution_x":500,"image_resolution_y":500,"compression":"opaque_wsq","images":[{"finger_position":1,"view_count":1,"view_number":1,"quality":80,"impression":0,"width":2,"height":2,"data":[0,1,2]}]});
    let input = data(&record).unwrap();
    let config = br#"{"engine_path":"missing"}"#;
    let mut output = 99;
    assert_eq!(
        unsafe {
            uareu_image_assess_record_view(input, 0, config.as_ptr(), config.len(), &mut output)
        },
        OK
    );
    let result: serde_json::Value = read_data(output).unwrap();
    assert_eq!(result["status"], "unsupported_input");
    assert!(result["reason"].as_str().unwrap().contains("compressed"));
    assert_eq!(uareu_data_free(output), OK);
    let config = br#"{"ppi":500}"#;
    assert_eq!(
        unsafe {
            uareu_image_assess_record_view(input, 0, config.as_ptr(), config.len(), &mut output)
        },
        INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
    assert_eq!(uareu_data_free(input), OK);
}

#[test]
fn target_wsq_rejects_bounds_before_image_lookup_or_codec() {
    for config in [
        serde_json::json!({"encoder":"missing","decoder":"missing","target_bytes":0}),
        serde_json::json!({"encoder":"missing","decoder":"missing","target_bytes":4194305}),
        serde_json::json!({"encoder":"missing","decoder":"missing","target_bytes":1024,"max_encodes":0}),
        serde_json::json!({"encoder":"missing","decoder":"missing","target_bytes":1024,"max_encodes":33}),
        serde_json::json!({"encoder":"missing","decoder":"missing","target_bytes":1024,"timeout_ms":0}),
    ] {
        let json = serde_json::to_vec(&config).unwrap();
        let mut output = 99;
        assert_eq!(
            unsafe { uareu_image_encode_wsq_target(0, json.as_ptr(), json.len(), &mut output) },
            INVALID_ARGUMENT
        );
        assert_eq!(output, 0);
    }
}

#[test]
fn status_wait_releases_state_lock_and_observes_changes() {
    let state = Arc::new(Mutex::new(ReaderState {
        closed: false,
        idle_reader: None,
        active: None,
        monitor: ReaderMonitor::default(),
        persistent: false,
    }));
    let sequence = state.lock().unwrap().monitor.snapshot().sequence;
    assert!(wait_reader_status(&state, sequence, 0).unwrap().is_none());
    let waiter_state = Arc::clone(&state);
    let waiter = std::thread::spawn(move || {
        wait_reader_status(&waiter_state, sequence, 1000)
            .unwrap()
            .unwrap()
    });
    state.lock().unwrap().monitor.begin();
    let observed = waiter.join().unwrap();
    assert!(observed.sequence > sequence);
    assert!(observed.capture_active);
    let sequence = observed.sequence;
    let waiter_state = Arc::clone(&state);
    let waiter = std::thread::spawn(move || wait_reader_status(&waiter_state, sequence, 1000));
    state.lock().unwrap().close();
    match waiter.join().unwrap() {
        Ok(Some(observed)) => {
            assert_eq!(observed.operation, crate::reader::ReaderOperation::Closed)
        }
        Err(Error(INVALID_HANDLE, _, _)) => (), // close preceded initial handle lookup
        other => panic!("unexpected close result: {other:?}"),
    }
}

#[test]
fn additive_record_abi_roundtrips_and_zeroes_failures() {
    let views = serde_json::json!({"width":20,"height":20,"pixels_per_cm_x":197,"pixels_per_cm_y":197,"vendor_id":0,"subformat":0,"device_info":0,"views":[{"finger_position":1,"view_number":0,"impression":0,"quality":80,"minutiae":[]},{"finger_position":1,"view_number":1,"impression":0,"quality":70,"minutiae":[]}]});
    for format in [0, 1] {
        let mut views = views.clone();
        views["vendor_id"] = serde_json::json!(format);
        let input = data(&views).unwrap();
        let mut encoded = 99;
        let mut decoded = 99;
        assert_eq!(
            unsafe { uareu_template_encode_views(input, format, &mut encoded) },
            OK
        );
        assert_eq!(
            unsafe { uareu_template_decode_views(encoded, format, &mut decoded) },
            OK
        );
        assert_eq!(read_data::<serde_json::Value>(decoded).unwrap(), views);
        for handle in [input, encoded, decoded] {
            assert_eq!(uareu_data_free(handle), OK);
            assert_eq!(uareu_data_free(handle), INVALID_HANDLE);
        }
    }
    let record = serde_json::json!({"capture_device_id":1,"acquisition_level":0,"scale_units":1,"scan_resolution_x":500,"scan_resolution_y":500,"image_resolution_x":500,"image_resolution_y":500,"compression":"raw8_bit","images":[{"finger_position":1,"view_count":2,"view_number":1,"quality":80,"impression":0,"width":2,"height":2,"data":[0,255,128,64]},{"finger_position":1,"view_count":2,"view_number":2,"quality":70,"impression":0,"width":2,"height":2,"data":[64,128,255,0]}]});
    let input = data(&record).unwrap();
    let mut encoded = 99;
    let mut decoded = 99;
    assert_eq!(
        unsafe { uareu_image_encode_images(input, &mut encoded) },
        OK
    );
    assert_eq!(
        unsafe { uareu_image_decode_images(encoded, &mut decoded) },
        OK
    );
    assert_eq!(read_data::<serde_json::Value>(decoded).unwrap(), record);
    for handle in [input, encoded, decoded] {
        assert_eq!(uareu_data_free(handle), OK);
    }
    let malformed = data(&vec![0u8, 1, 2]).unwrap();
    for format in [0, 1] {
        let mut output = 99;
        assert_eq!(
            unsafe { uareu_template_decode_views(malformed, format, &mut output) },
            IMAGE_ERROR
        );
        assert_eq!(output, 0);
    }
    let mut output = 99;
    assert_eq!(
        unsafe { uareu_image_decode_images(malformed, &mut output) },
        IMAGE_ERROR
    );
    assert_eq!(output, 0);
    output = 99;
    assert_eq!(
        unsafe { uareu_template_encode_views(malformed, 2, &mut output) },
        INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
    assert_eq!(uareu_data_free(malformed), OK);
    assert_eq!(
        unsafe { uareu_image_encode_images(0, &mut output) },
        INVALID_HANDLE
    );
    assert_eq!(output, 0);
    let mut completed = 99;
    output = 99;
    assert_eq!(
        unsafe { uareu_reader_wait_status_json(0, 0, 0, &mut completed, &mut output) },
        INVALID_HANDLE
    );
    assert_eq!((completed, output), (0, 0));
}

#[test]
fn structured_error_getters_preserve_error_and_legacy_codes() {
    for (kind, legacy) in [
        (CaptureErrorKind::Timeout, DEVICE_ERROR),
        (CaptureErrorKind::Disconnected, DEVICE_ERROR),
        (CaptureErrorKind::Cancelled, CANCELLED),
    ] {
        assert_eq!(
            boundary(|| Err(mapped(
                legacy,
                CaptureError::new(kind, CaptureStage::FingerWait, "injected cause").into()
            ))),
            legacy
        );
        let mut required = 0;
        assert_eq!(
            unsafe { uareu_last_error_json(null_mut(), 0, &mut required) },
            BUFFER_TOO_SMALL
        );
        let mut tiny = [99u8; 1];
        assert_eq!(
            unsafe { uareu_last_error_json(tiny.as_mut_ptr(), 1, &mut required) },
            BUFFER_TOO_SMALL
        );
        assert_eq!(tiny, [99]);
        let mut bytes = vec![0; required];
        assert_eq!(
            unsafe { uareu_last_error_json(bytes.as_mut_ptr(), bytes.len(), &mut required) },
            OK
        );
        let report: serde_json::Value = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(report["status"], legacy);
        assert_eq!(report["kind"], serde_json::to_value(kind).unwrap());
        assert_eq!(report["stage"], "finger_wait");
        assert!(last_error().contains("injected cause"));
        std::thread::spawn(|| {
            assert_eq!(
                boundary(|| Err(error(INVALID_ARGUMENT, "other thread"))),
                INVALID_ARGUMENT
            );
            assert_eq!(
                LAST_ERROR.with(|last| last.borrow().kind.clone()),
                "invalid_argument"
            );
        })
        .join()
        .unwrap();
        assert!(last_error().contains("injected cause"));
    }
    assert_eq!(boundary(|| Ok(())), OK);
    assert_eq!(LAST_ERROR.with(|last| last.borrow().kind.clone()), "none");
    let mut required = 123;
    assert_eq!(
        unsafe { uareu_reader_get_status_json(0, null_mut(), 0, &mut required) },
        INVALID_HANDLE
    );
    assert_eq!(required, 0);
    assert_eq!(
        unsafe { uareu_reader_get_capabilities_json(0, null_mut(), 0, &mut required) },
        INVALID_HANDLE
    );
    assert_eq!(
        unsafe { uareu_reader_list_get_capabilities_json(0, 0, null_mut(), 0, &mut required) },
        INVALID_HANDLE
    );
}

#[test]
fn image_abi_preserves_pixels_metadata_and_ownership() {
    let captured = crate::frame::decode_frame_with_metadata(
        include_bytes!("../../fixtures/synthetic-plain.wire"),
        true,
        |_| unreachable!(),
    )
    .unwrap();
    let expected_pixels = captured.image.pixels.clone();
    let expected_metadata = serde_json::to_value(&captured.metadata).unwrap();
    let id = register_image(captured).unwrap();
    let mut info = ImageInfo::default();
    assert_eq!(unsafe { uareu_image_get_info(id, &mut info, 16) }, OK);
    assert_eq!(
        (
            info.width,
            info.height,
            info.stride_bytes,
            info.bits_per_pixel
        ),
        (384, 290, 384, 8)
    );
    assert_eq!(
        unsafe { uareu_image_get_info(id, &mut info, 15) },
        INVALID_ARGUMENT
    );

    let mut required = 0;
    assert_eq!(
        unsafe { uareu_image_get_pixels(id, null_mut(), 0, &mut required) },
        BUFFER_TOO_SMALL
    );
    assert_eq!(required, expected_pixels.len());
    let mut tiny = [123; 2];
    assert_eq!(
        unsafe { uareu_image_get_pixels(id, tiny.as_mut_ptr(), tiny.len(), &mut required) },
        BUFFER_TOO_SMALL
    );
    assert_eq!(tiny, [123; 2]);
    let mut pixels = vec![0; required];
    assert_eq!(
        unsafe { uareu_image_get_pixels(id, pixels.as_mut_ptr(), pixels.len(), &mut required) },
        OK
    );
    assert_eq!(pixels, expected_pixels);
    assert_eq!(
        unsafe { uareu_image_get_report_json(id, null_mut(), 0, &mut required) },
        BUFFER_TOO_SMALL
    );
    let mut report = vec![0; required];
    assert_eq!(
        unsafe {
            uareu_image_get_report_json(id, report.as_mut_ptr(), report.len(), &mut required)
        },
        OK
    );
    assert_eq!(report.last(), Some(&0));
    let report: serde_json::Value = serde_json::from_slice(&report[..report.len() - 1]).unwrap();
    assert_eq!(report["metadata"], expected_metadata);
    assert_eq!(report["quality"]["status"], "not_evaluated");
    let mut count = 123;
    assert_eq!(
        unsafe { uareu_reader_list_count(id, &mut count) },
        INVALID_HANDLE
    );
    assert_eq!(count, 0);
    assert_eq!(uareu_reader_list_free(id), INVALID_HANDLE);
    let retained = image(id).unwrap();
    assert_eq!(uareu_image_free(id), OK);
    assert_eq!(uareu_image_free(id), INVALID_HANDLE);
    assert_eq!(retained.captured.image.pixels, expected_pixels);
}

#[test]
fn invalid_arguments_fail_before_usb_and_errors_are_thread_local() {
    assert_eq!(
        unsafe { uareu_reader_list_create(null_mut()) },
        INVALID_ARGUMENT
    );
    let mut storage = [0u64; 2];
    let misaligned = unsafe { storage.as_mut_ptr().cast::<u8>().add(1).cast::<u64>() };
    assert_eq!(
        unsafe { uareu_reader_list_create(misaligned) },
        INVALID_ARGUMENT
    );
    let mut output = 123;
    assert_eq!(
        unsafe { uareu_capture(0, 0, 1, &mut output) },
        INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
    assert!(last_error().contains("timeout_ms"));
    std::thread::spawn(|| {
        assert_eq!(uareu_image_free(0), INVALID_HANDLE);
        assert!(last_error().contains("handle"));
    })
    .join()
    .unwrap();
    assert!(last_error().contains("timeout_ms"));
    let mut required = 0;
    assert_eq!(
        unsafe { uareu_last_error(null_mut(), 99, &mut required) },
        INVALID_ARGUMENT
    );
    assert!(last_error().contains("timeout_ms"));
    assert_eq!(
        unsafe { uareu_image_load_png([255].as_ptr(), 1, &mut output) },
        INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { uareu_image_load_png(b"bad\0path".as_ptr(), 8, &mut output) },
        INVALID_ARGUMENT
    );
}

#[test]
fn capture_state_serializes_operations_and_close_cancels_active_work() {
    let shared = Arc::new(Mutex::new(ReaderState {
        closed: false,
        idle_reader: None,
        active: None,
        monitor: ReaderMonitor::default(),
        persistent: false,
    }));
    shared.lock().unwrap().cancel().unwrap();
    let (first, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
    let monitor = shared.lock().unwrap().monitor.clone();
    assert!(monitor.snapshot().capture_active);
    assert!(!first.cancel.load(Ordering::Relaxed));
    assert!(matches!(
        CaptureGuard::begin(Arc::clone(&shared)),
        Err(Error(BUSY, _, _))
    ));
    let other = Arc::clone(&shared);
    std::thread::spawn(move || other.lock().unwrap().cancel().unwrap())
        .join()
        .unwrap();
    assert!(first.cancel.load(Ordering::Relaxed));
    drop(first);
    assert!(!monitor.snapshot().capture_active);
    let (second, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
    assert!(
        !second.cancel.load(Ordering::Relaxed),
        "a previous cancellation must not affect the next call"
    );
    shared.lock().unwrap().close();
    assert_eq!(
        monitor.snapshot().operation,
        crate::reader::ReaderOperation::Closed
    );
    assert!(second.cancel.load(Ordering::Relaxed));
    drop(second);
    assert!(matches!(
        CaptureGuard::begin(Arc::clone(&shared)),
        Err(Error(INVALID_HANDLE, _, _))
    ));
}

#[test]
fn cleanup_disconnect_discards_session_without_replacing_primary_error() {
    let primary: anyhow::Error = CaptureError::new(
        CaptureErrorKind::Timeout,
        CaptureStage::Acquisition,
        "deadline",
    )
    .into();
    assert!(!is_disconnected(&primary));
    let combined = crate::error::attach_cleanup(
        primary,
        Err(CaptureError::new(
            CaptureErrorKind::Disconnected,
            CaptureStage::Cleanup,
            "reader unplugged",
        )
        .into()),
    );
    let combined = crate::error::attach_cleanup(
        combined,
        Err(CaptureError::new(
            CaptureErrorKind::Transport,
            CaptureStage::Cleanup,
            "queue drain failed",
        )
        .into()),
    );
    assert!(is_disconnected(&combined));
    let report = CaptureFailure::from_error(&combined);
    assert_eq!(report.kind, CaptureErrorKind::Timeout);
    let first = report.cleanup.unwrap();
    assert_eq!(first.kind, CaptureErrorKind::Disconnected);
    assert_eq!(first.cleanup.unwrap().kind, CaptureErrorKind::Transport);
    assert_eq!(capture_error(combined).0, DEVICE_ERROR);
}
#[test]
fn panics_are_contained_and_capture_lease_is_released() {
    let shared = Arc::new(Mutex::new(ReaderState {
        closed: false,
        idle_reader: None,
        active: None,
        monitor: ReaderMonitor::default(),
        persistent: false,
    }));
    assert_eq!(
        boundary(|| {
            let (_guard, _) = CaptureGuard::begin(Arc::clone(&shared))?;
            panic!("injected FFI failure");
        }),
        PANIC
    );
    assert!(last_error().contains("panic contained"));
    assert!(shared.lock().unwrap().active.is_none());
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .monitor
            .snapshot()
            .last_outcome
            .unwrap()
            .failure
            .unwrap()
            .kind,
        CaptureErrorKind::Panic
    );
    assert!(CaptureGuard::begin(shared.clone()).is_ok());
    assert_eq!(
        boundary(|| {
            let (_guard, _) = CaptureGuard::begin_mode(shared.clone(), false)?;
            panic!("injected probe failure");
        }),
        PANIC
    );
    let state = shared.lock().unwrap();
    assert!(state.active.is_none());
    let status = state.monitor.snapshot();
    assert_eq!(status.operation, crate::reader::ReaderOperation::Idle);
    assert_eq!(
        status.last_probe_failure.unwrap().kind,
        CaptureErrorKind::Panic
    );
}
