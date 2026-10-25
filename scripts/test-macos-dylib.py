#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Load the actual macOS cdylib and exercise device-free ABI calls.

Usage: python3 scripts/test-macos-dylib.py target/release/libuareu.dylib
Optional verification/CI tool; Python is not required to build or run the SDK.
No reader interface is opened and no USB register commands are issued.
"""

import ctypes
import pathlib
import sys


def main():
    if sys.platform != "darwin":
        raise SystemExit("this test requires a native macOS host")
    library = pathlib.Path(sys.argv[1]).resolve(strict=True)
    sdk = ctypes.CDLL(str(library))
    sdk.uareu_abi_version.argtypes = []
    sdk.uareu_abi_version.restype = ctypes.c_uint32
    assert sdk.uareu_abi_version() == 1, "unexpected ABI version"
    sdk.uareu_last_error.argtypes = [
        ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    sdk.uareu_last_error.restype = ctypes.c_uint32
    required = ctypes.c_size_t()
    assert sdk.uareu_last_error(None, 0, ctypes.byref(required)) == 3
    assert required.value > 0
    buffer = (ctypes.c_uint8 * required.value)()
    assert sdk.uareu_last_error(buffer, len(buffer), ctypes.byref(required)) == 0
    assert buffer[required.value - 1] == 0, "error buffer must be NUL terminated"
    print(f"PASS: native macOS dylib load, ABI v1, caller-owned error buffer: {library}")


if __name__ == "__main__":
    main()
