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
    REQUIRE(sizeof(uareu_image_info) == 16);
    REQUIRE(uareu_abi_version() == UAREU_ABI_VERSION);
    REQUIRE(uareu_capture_with_callback(0, 30000, 1, NULL, NULL, &image) == UAREU_INVALID_HANDLE);
    REQUIRE(image == 0);
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
    check(uareu_reader_list_free(list));
    REQUIRE(uareu_reader_list_count(list, &count) == UAREU_INVALID_HANDLE);
    puts("C ABI smoke test passed: layout, buffers, pixels, metadata, diagnostics, discovery, stale handles.");
    return 0;
}
