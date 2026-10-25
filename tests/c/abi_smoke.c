/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Native ABI smoke test: synthetic image, discovery only, no finger capture. */
#include "uareu.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void check(uareu_status status) {
    if (status != UAREU_OK) {
        uint8_t error[2048];
        size_t required = 0;
        if (uareu_last_error(error, sizeof(error), &required) == UAREU_OK)
            fprintf(stderr, "status %u: %s\n", (unsigned)status, (const char *)error);
        exit(1);
    }
}
#define REQUIRE(condition) do { if (!(condition)) { fprintf(stderr, "Assertion failed at line %d\n", __LINE__); exit(1); } } while (0)

static unsigned completion_calls = 0;
static void UAREU_CALL completion_check(uareu_status status, uareu_handle image, uareu_handle report, void *context) {
    (void)status; (void)context;
    ++completion_calls;
    if (image) check(uareu_image_free(image));
    if (report) check(uareu_data_free(report));
}

static void check_ansi381_and_completion_contract(void) {
    static const uint8_t ansi[] = "{\"cbeff_product_id\":65537,\"appendix_f_compliant\":false,\"record\":{\"capture_device_id\":1,\"acquisition_level\":10,\"scale_units\":1,\"scan_resolution_x\":500,\"scan_resolution_y\":500,\"image_resolution_x\":500,\"image_resolution_y\":500,\"compression\":\"raw8_bit\",\"images\":[{\"finger_position\":1,\"view_count\":1,\"view_number\":1,\"quality\":254,\"impression\":0,\"width\":2,\"height\":2,\"data\":[0,255,128,64]}]}}";
    uareu_handle input=0, bytes=0, record=0, image=0, task=99;
    uint8_t pixels[4]; size_t required=0;
    REQUIRE(uareu_capture_start_with_completion(0,100,1,NULL,NULL,&task) == UAREU_INVALID_ARGUMENT);
    REQUIRE(task == 0);
    task=99;
    REQUIRE(uareu_capture_start_with_completion(0,100,1,completion_check,NULL,&task) == UAREU_INVALID_HANDLE);
    REQUIRE(task == 0 && completion_calls == 0);
    check(uareu_data_from_json(ansi,sizeof(ansi)-1,&input));
    check(uareu_image_encode_ansi381(input,&bytes));
    check(uareu_image_decode_ansi381(bytes,&record));
    check(uareu_image_ansi381_view(record,0,&image));
    check(uareu_image_get_pixels(image,pixels,sizeof(pixels),&required));
    REQUIRE(required == 4 && pixels[0] == 0 && pixels[1] == 255 && pixels[2] == 128 && pixels[3] == 64);
    check(uareu_data_free(input)); check(uareu_data_free(bytes)); check(uareu_data_free(record)); check(uareu_image_free(image));
}

static void check_additive_records(void) {
    static const uint8_t views[] = "{\"width\":20,\"height\":20,\"pixels_per_cm_x\":197,\"pixels_per_cm_y\":197,\"vendor_id\":1,\"subformat\":0,\"device_info\":0,\"views\":[{\"finger_position\":1,\"view_number\":0,\"impression\":0,\"quality\":80,\"minutiae\":[]},{\"finger_position\":1,\"view_number\":1,\"impression\":0,\"quality\":70,\"minutiae\":[]}]}";
    static const uint8_t images[] = "{\"capture_device_id\":1,\"acquisition_level\":0,\"scale_units\":1,\"scan_resolution_x\":500,\"scan_resolution_y\":500,\"image_resolution_x\":500,\"image_resolution_y\":500,\"compression\":\"raw8_bit\",\"images\":[{\"finger_position\":1,\"view_count\":1,\"view_number\":1,\"quality\":80,\"impression\":0,\"width\":2,\"height\":2,\"data\":[0,255,128,64]}]}";
    static const uint8_t malformed[] = "[0,1,2]";
    uint32_t format, completed = 99;
    uareu_handle input = 0, encoded = 0, decoded = 0, output = 99;
    uint8_t json[4096];
    size_t required = 0;
    REQUIRE(uareu_reader_wait_status_json(0, 0, 0, &completed, &output) == UAREU_INVALID_HANDLE);
    REQUIRE(completed == 0 && output == 0);
    for (format = UAREU_TEMPLATE_ISO_19794_2_2005; format <= UAREU_TEMPLATE_ANSI_378_2004; ++format) {
        check(uareu_data_from_json(views, sizeof(views)-1, &input));
        check(uareu_template_encode_views(input, format, &encoded));
        check(uareu_template_decode_views(encoded, format, &decoded));
        check(uareu_data_get_json(decoded, json, sizeof(json), &required));
        REQUIRE(strstr((const char *)json, "\"view_number\":1") != NULL);
        check(uareu_data_free(input)); check(uareu_data_free(encoded)); check(uareu_data_free(decoded));
        REQUIRE(uareu_data_free(decoded) == UAREU_INVALID_HANDLE);
    }
    {
        char extended[4096];
        int length = snprintf(extended, sizeof(extended),
            "{\"record\":%s,\"extensions\":[[{\"type_code\":257,\"data\":[0,255,17]}],[]]}", (const char *)views);
        REQUIRE(length > 0 && (size_t)length < sizeof(extended));
        check(uareu_data_from_json((const uint8_t *)extended, (size_t)length, &input));
        for (format = 0; format <= 1; ++format) {
            check(uareu_template_encode_extended(input, format, &encoded));
            check(uareu_template_decode_extended(encoded, format, &decoded));
            check(uareu_data_get_json(decoded, json, sizeof(json), &required));
            REQUIRE(strstr((const char *)json, "[0,255,17]") != NULL);
            output = 99;
            REQUIRE(uareu_template_decode_views(encoded, format, &output) == UAREU_IMAGE_ERROR);
            REQUIRE(output == 0);
            check(uareu_data_free(encoded)); check(uareu_data_free(decoded));
        }
        check(uareu_data_free(input));
    }
    check(uareu_data_from_json(images, sizeof(images)-1, &input));
    check(uareu_image_assess_record_view(input, 0, (const uint8_t *)"{}", 2, &output));
    check(uareu_data_get_json(output, json, sizeof(json), &required));
    REQUIRE(strstr((const char *)json, "\"status\":\"not_evaluated\"") != NULL);
    check(uareu_data_free(output));
    check(uareu_image_encode_images(input, &encoded));
    check(uareu_image_decode_images(encoded, &decoded));
    check(uareu_data_get_json(decoded, json, sizeof(json), &required));
    REQUIRE(strstr((const char *)json, "[0,255,128,64]") != NULL);
    check(uareu_data_free(input)); check(uareu_data_free(encoded)); check(uareu_data_free(decoded));
    check(uareu_data_from_json(malformed, sizeof(malformed)-1, &input));
    output = 99;
    REQUIRE(uareu_template_decode_views(input, 0, &output) == UAREU_IMAGE_ERROR);
    REQUIRE(output == 0);
    output = 99;
    REQUIRE(uareu_image_decode_images(input, &output) == UAREU_IMAGE_ERROR);
    REQUIRE(output == 0);
    check(uareu_data_free(input));
}

/* Optional real-tool native integration; the normal smoke needs no backends.
 * Tests deliberately use synthetic pixels and explicit caller-declared scale. */
static void check_optional_codecs_and_legacy_quality(void) {
    const char *encoder = getenv("UAREU_OPENJPEG_ENCODER");
    const char *decoder = getenv("UAREU_OPENJPEG_DECODER");
    const char *nfiq = getenv("UAREU_NFIQ1_EXE");
    uint8_t pixels[65536], decoded[65536], json[4096];
    char config[8192];
    uareu_handle image = 0, bytes = 0, restored = 0, report = 0;
    size_t i, required = 0;
    int length;
    if (!encoder && !decoder && !nfiq) return;
    for (i = 0; i < sizeof(pixels); ++i) pixels[i] = (uint8_t)((i * 17 + i / 256) % 256);
    check(uareu_image_from_pixels(256,256,pixels,sizeof(pixels),NULL,0,&image));
    if (encoder || decoder) {
        REQUIRE(encoder && decoder);
        /* Supplied local paths must be JSON-safe; no shell is involved. */
        REQUIRE(!strchr(encoder,'"') && !strchr(encoder,'\\'));
        REQUIRE(!strchr(decoder,'"') && !strchr(decoder,'\\'));
        length = snprintf(config,sizeof(config),"{\"encoder\":\"%s\",\"decoder\":\"%s\"}",encoder,decoder);
        REQUIRE(length > 0 && (size_t)length < sizeof(config));
        check(uareu_image_encode_jpeg2000(image,(const uint8_t *)config,(size_t)length,&bytes));
        check(uareu_image_decode_jpeg2000(bytes,(const uint8_t *)config,(size_t)length,&restored));
        check(uareu_image_get_pixels(restored,decoded,sizeof(decoded),&required));
        REQUIRE(required == sizeof(pixels) && memcmp(pixels,decoded,sizeof(pixels)) == 0);
        check(uareu_image_get_report_json(restored,json,sizeof(json),&required));
        REQUIRE(strstr((const char *)json,"\"status\":\"unknown\"") != NULL);
        check(uareu_data_free(bytes)); check(uareu_image_free(restored));
    }
    if (nfiq) {
        REQUIRE(!strchr(nfiq,'"') && !strchr(nfiq,'\\'));
        length = snprintf(config,sizeof(config),"{\"engine_path\":\"%s\",\"ppi\":500}",nfiq);
        REQUIRE(length > 0 && (size_t)length < sizeof(config));
        check(uareu_image_assess_nfiq1(image,(const uint8_t *)config,(size_t)length,&report));
        check(uareu_data_get_json(report,json,sizeof(json),&required));
        REQUIRE(strstr((const char *)json,"\"status\":\"scored\"") != NULL);
        REQUIRE(strstr((const char *)json,"\"evaluation_ppi\":500") != NULL);
        check(uareu_data_free(report));
    }
    check(uareu_image_free(image));
}

int main(int argc, char **argv) {
    /* Generated from deterministic non-biometric pixels [0,255,128,64]. */
    static const uint8_t png[] = {
        0x89,0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a,0,0,0,0x0d,0x49,0x48,0x44,0x52,
        0,0,0,2,0,0,0,2,8,0,0,0,0,0x57,0xdd,0x52,0xf8,0,0,0,0x0e,0x49,0x44,0x41,
        0x54,0x78,0x9c,0x63,0x61,0xf8,0xcf,0xd8,0x70,0,0,5,0xdd,2,0x45,0xcd,0x25,
        0x5f,0x3e,0,0,0,0,0x49,0x45,0x4e,0x44,0xae,0x42,0x60,0x82
    };
    FILE *file = NULL;
    uareu_handle image = 0, list = 0;
    uareu_image_info info;
    uint32_t count = 0;
    size_t required = 0;
    uint8_t pixels[4] = { 0 };
    uint8_t tiny[2] = { 42, 42 };
    uint8_t *report;
    REQUIRE(argc == 2);
    check_additive_records();
    check_ansi381_and_completion_contract();
    check_optional_codecs_and_legacy_quality();
    {
        uareu_handle output = 99;
        REQUIRE(uareu_image_flat_field_profile(0, 0, 8.0, &output) == UAREU_INVALID_HANDLE);
        REQUIRE(output == 0);
        output = 99;
        REQUIRE(uareu_image_apply_flat_field(0, 0, &output) == UAREU_INVALID_HANDLE);
        REQUIRE(output == 0);
    }
    REQUIRE(sizeof(uareu_image_info) == 16);
    REQUIRE(uareu_abi_version() == UAREU_ABI_VERSION);
    REQUIRE(uareu_capture_with_callback(0, 30000, 1, NULL, NULL, &image) == UAREU_INVALID_HANDLE);
    REQUIRE(image == 0);
    {
        uint8_t details[2048];
        REQUIRE(uareu_last_error_json(NULL, 0, &required) == UAREU_BUFFER_TOO_SMALL);
        REQUIRE(required <= sizeof(details));
        check(uareu_last_error_json(details, sizeof(details), &required));
        REQUIRE(strstr((const char *)details, "\"kind\":\"invalid_handle\"") != NULL);
        REQUIRE(strstr((const char *)details, "\"status\":2") != NULL);
        check(uareu_last_error(details, sizeof(details), &required));
        REQUIRE(strstr((const char *)details, "handle") != NULL);
        REQUIRE(uareu_reader_get_status_json(0, NULL, 0, &required) == UAREU_INVALID_HANDLE);
        REQUIRE(uareu_reader_get_capabilities_json(0, NULL, 0, &required) == UAREU_INVALID_HANDLE);
        REQUIRE(uareu_reader_list_get_capabilities_json(0, 0, NULL, 0, &required) == UAREU_INVALID_HANDLE);
    }
    /* The caller must supply a fresh test-output path. */
    REQUIRE(fopen_s(&file, argv[1], "wbx") == 0 && file != NULL);
    REQUIRE(fwrite(png, 1, sizeof(png), file) == sizeof(png));
    REQUIRE(fclose(file) == 0);
    check(uareu_image_load_png((const uint8_t *)argv[1], strlen(argv[1]), &image));
    REQUIRE(image != 0);
    check(uareu_image_get_info(image, &info, sizeof(info)));
    REQUIRE(info.width == 2 && info.height == 2 && info.stride_bytes == 2 && info.bits_per_pixel == 8);
    REQUIRE(uareu_image_get_pixels(image, NULL, 0, &required) == UAREU_BUFFER_TOO_SMALL && required == 4);
    REQUIRE(uareu_image_get_pixels(image, tiny, sizeof(tiny), &required) == UAREU_BUFFER_TOO_SMALL);
    REQUIRE(tiny[0] == 42 && tiny[1] == 42);
    check(uareu_image_get_pixels(image, pixels, sizeof(pixels), &required));
    REQUIRE(pixels[0] == 0 && pixels[1] == 255 && pixels[2] == 128 && pixels[3] == 64);
    REQUIRE(uareu_image_get_report_json(image, NULL, 0, &required) == UAREU_BUFFER_TOO_SMALL);
    report = (uint8_t *)malloc(required);
    REQUIRE(report != NULL);
    check(uareu_image_get_report_json(image, report, required, &required));
    REQUIRE(strstr((const char *)report, "\"status\":\"unknown\"") != NULL);
    REQUIRE(strstr((const char *)report, "\"mean\":111.75") != NULL);
    REQUIRE(strstr((const char *)report, "\"status\":\"not_evaluated\"") != NULL);
    free(report);
    REQUIRE(uareu_reader_list_free(image) == UAREU_INVALID_HANDLE);
    check(uareu_image_free(image));
    REQUIRE(uareu_image_free(image) == UAREU_INVALID_HANDLE);
    check(uareu_reader_list_create(&list));
    check(uareu_reader_list_count(list, &count));
    if (count > 0) {
        uint8_t capabilities[4096];
        REQUIRE(uareu_reader_list_get_capabilities_json(list, 0, NULL, 0, &required) == UAREU_BUFFER_TOO_SMALL);
        REQUIRE(required <= sizeof(capabilities));
        check(uareu_reader_list_get_capabilities_json(list, 0, capabilities, sizeof(capabilities), &required));
        REQUIRE(strstr((const char *)capabilities, "\"resolutions_ppi\":null") != NULL);
        REQUIRE(strstr((const char *)capabilities, "\"calibration\":\"unknown\"") != NULL);
    }
    check(uareu_reader_list_free(list));
    REQUIRE(uareu_reader_list_count(list, &count) == UAREU_INVALID_HANDLE);
    puts("C ABI smoke test passed: layout, buffers, pixels, metadata, diagnostics, discovery, additive records, stale handles.");
    return 0;
}
