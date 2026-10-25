// SPDX-License-Identifier: GPL-3.0-or-later
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        try
        {
            string? dllPath = Environment.GetEnvironmentVariable("UAREU_DLL_PATH");
            if (!string.IsNullOrWhiteSpace(dllPath))
            {
                // Retain the native library for the process lifetime. Every handle
                // below is released before Main returns; never unload during capture.
                nint library = NativeLibrary.Load(Path.GetFullPath(dllPath));
                NativeLibrary.SetDllImportResolver(typeof(Native).Assembly,
                    (name, _, _) => name == "uareu" ? library : 0);
            }
            if (Native.uareu_abi_version() != 1)
                throw new InvalidOperationException("This example requires ABI version 1.");
            if (args.Length > 0 && Advanced.Commands.Contains(args[0])) return await Advanced.Run(args);

            if (args.Length == 1 && args[0] == "--self-test")
            {
                Native.VerifySnapshotBuffers();
                Console.WriteLine("Managed snapshot buffers and structured errors verified.");
                return 0;
            }

            if (args.Length == 2 && args[0] == "--inspect")
            {
                byte[] path = Encoding.UTF8.GetBytes(args[1]);
                Native.Check(Native.uareu_image_load_png(path, (nuint)path.Length, out ulong image));
                try { ShowImage(image); }
                finally { Native.Check(Native.uareu_image_free(image)); }
                return 0;
            }
            bool listOnly = args.Length == 1 && args[0] == "--list";
            bool cancelTest = args.Length == 1 && args[0] == "--cancel-test";
            bool statusOnly = args.Length == 1 && args[0] == "--status";
            bool timeoutTest = args.Length == 1 && args[0] == "--timeout-test";
            bool phaseTest = args.Length == 1 && args[0] == "--phase-test";
            bool capture = args.Length is 2 or 3 && args[0] == "--capture";
            if (!listOnly && !cancelTest && !capture && !statusOnly && !timeoutTest && !phaseTest)
            {
                Console.Error.WriteLine("Usage: --list | --status | --inspect image.png | --capture new.png [reader-index] | --cancel-test | --timeout-test | --phase-test | --self-test");
                return 2;
            }

            Native.Check(Native.uareu_reader_list_create(out ulong list));
            ulong reader = 0;
            try
            {
                Native.Check(Native.uareu_reader_list_count(list, out uint count));
                Console.WriteLine($"Supported readers: {count}");
                for (uint index = 0; index < count; index++)
                {
                    uint selected = index;
                    Console.WriteLine(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
                        Native.uareu_reader_list_get_json(list, selected, buffer, capacity, out required)));
                    Console.WriteLine(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
                        Native.uareu_reader_list_get_capabilities_json(list, selected, buffer, capacity, out required)));
                }
                if (listOnly)
                    return 0;
                if (count == 0)
                    throw new InvalidOperationException("Connect the reader and assign WinUSB before capturing.");
                uint readerIndex = args.Length == 3 ? uint.Parse(args[2]) : 0;
                Native.Check(Native.uareu_reader_open(list, readerIndex, out reader));
            }
            finally { Native.Check(Native.uareu_reader_list_free(list)); }

            try
            {
                ShowStatus(reader);
                Console.WriteLine(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
                    Native.uareu_reader_get_capabilities_json(reader, buffer, capacity, out required)));
                if (statusOnly)
                {
                    using JsonDocument snapshot = JsonDocument.Parse(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
                        Native.uareu_reader_get_status_json(reader, buffer, capacity, out required)));
                    foreach (string name in new[] { "finger_present", "hardware_ready", "calibration_required", "connected" })
                        if (snapshot.RootElement.GetProperty(name).ValueKind != JsonValueKind.Null)
                            throw new InvalidOperationException($"Fresh status must not guess {name}.");
                    if (snapshot.RootElement.GetProperty("busy").GetBoolean())
                        throw new InvalidOperationException("Fresh reader must not be busy.");
                    return 0;
                }
                Native.Check(Native.uareu_cancel(reader)); // Idle cancellation is harmless.
                ConsoleCancelEventHandler cancelHandler = (_, signal) =>
                {
                    signal.Cancel = true;
                    Native.uareu_cancel(reader);
                };
                Console.CancelKeyPress += cancelHandler;
                try
                {
                    Task<(uint Status, ulong Image, Native.ErrorReport? Error)> worker = Task.Run(() =>
                    {
                        Exception? callbackFailure = null;
                        bool waitingObserved = false;
                        Native.CaptureCallback progress = (phase, _) =>
                        {
                            // Managed exceptions must never cross a native callback.
                            try
                            {
                                // Force expiry at a notification boundary, before
                                // any protocol writes; no finger is needed.
                                if (timeoutTest) { Thread.Sleep(20); return; }
                                if (phaseTest)
                                {
                                    using JsonDocument snapshot = JsonDocument.Parse(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
                                        Native.uareu_reader_get_status_json(reader, buffer, capacity, out required)));
                                    string expected = phase switch { 1 => "initializing", 2 => "waiting_for_finger", 3 => "waiting_for_finger_removal", 4 => "capturing", _ => "unknown" };
                                    if (snapshot.RootElement.GetProperty("operation").GetString() != expected ||
                                        !snapshot.RootElement.GetProperty("capture_active").GetBoolean() ||
                                        !snapshot.RootElement.GetProperty("busy").GetBoolean())
                                        throw new InvalidOperationException("Capture status did not agree with its notification.");
                                    if (phase == 2)
                                    {
                                        waitingObserved = true;
                                        if (snapshot.RootElement.GetProperty("finger_present").ValueKind != JsonValueKind.Null)
                                            throw new InvalidOperationException("Waiting must not claim finger absence.");
                                        Native.Check(Native.uareu_cancel(reader));
                                    }
                                    return;
                                }
                                Console.WriteLine(phase switch
                                {
                                    1 => "Initializing reader...",
                                    2 => "Place your finger flat on the reader and hold it until saved (Ctrl-C cancels).",
                                    3 => "Lift your finger from the reader.",
                                    4 => "Capturing; keep your finger still...",
                                    _ => $"Capture phase {phase}"
                                });
                                ShowStatus(reader);
                            }
                            catch (Exception error)
                            {
                                // Save test failures to throw after native return.
                                // Ordinary console errors must not unwind into Rust.
                                if (phaseTest)
                                {
                                    callbackFailure = error;
                                    try { Native.uareu_cancel(reader); }
                                    catch { /* Never unwind a managed exception into Rust. */ }
                                }
                            }
                        };
                        uint status = cancelTest
                            ? Native.uareu_capture(reader, 30_000, 1, out ulong image)
                            : Native.uareu_capture_with_callback(reader, timeoutTest ? 1u : 30_000u, 1, progress, 0, out image);
                        // Copy any error while still on the native-calling thread.
                        Native.ErrorReport? error = status == Native.Ok ? null : Native.ReadError();
                        if (phaseTest && (callbackFailure != null || !waitingObserved))
                        {
                            if (image != 0) Native.Check(Native.uareu_image_free(image));
                            throw new InvalidOperationException("Capture phase/status verification failed.", callbackFailure);
                        }
                        return (status, image, error);
                    });
                    if (cancelTest)
                    {
                        // Repeat until the worker has entered capture; cancellation
                        // requested before capture starts is intentionally a no-op.
                        while (!worker.IsCompleted)
                        {
                            Native.Check(Native.uareu_cancel(reader));
                            await Task.WhenAny(worker, Task.Delay(10));
                        }
                    }
                    var result = await worker;
                    ShowStatus(reader);
                    if (result.Error != null) Console.WriteLine(JsonSerializer.Serialize(result.Error));
                    if (timeoutTest)
                    {
                        if (result.Status != 6 || result.Image != 0 || result.Error?.Kind != "timeout")
                        {
                            if (result.Image != 0) Native.Check(Native.uareu_image_free(result.Image));
                            throw new InvalidOperationException("Expected structured timeout with legacy DEVICE_ERROR status.");
                        }
                        Console.WriteLine("Structured timeout verified; legacy status preserved.");
                        return 0;
                    }
                    if (result.Status == Native.Cancelled)
                    {
                        if (result.Image != 0 || result.Error?.Kind != "cancelled")
                            throw new InvalidOperationException("Cancellation report did not match capture result.");
                        Console.WriteLine("Capture cancelled; reader cleanup completed.");
                        if (phaseTest) Console.WriteLine("Reentrant status and callback cancellation verified.");
                        return 0;
                    }
                    if (result.Status != Native.Ok)
                        throw new Native.NativeException(result.Status, result.Error!);
                    try
                    {
                        if (cancelTest || phaseTest)
                            throw new InvalidOperationException("Capture finished before cancellation; retry with no finger on the sensor.");
                        byte[] path = Encoding.UTF8.GetBytes(args[1]);
                        Native.Check(Native.uareu_image_save_png(result.Image, path, (nuint)path.Length));
                        Console.WriteLine($"Saved: {args[1]}");
                        ShowImage(result.Image);
                    }
                    finally { Native.Check(Native.uareu_image_free(result.Image)); }
                }
                finally { Console.CancelKeyPress -= cancelHandler; }
            }
            finally { Native.Check(Native.uareu_reader_close(reader)); }
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error.Message);
            return 1;
        }
    }

    private static void ShowStatus(ulong reader)
    {
        Console.WriteLine(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
            Native.uareu_reader_get_status_json(reader, buffer, capacity, out required)));
    }

    private static void ShowImage(ulong image)
    {
        Native.Check(Native.uareu_image_get_info(image, out var info, (nuint)Marshal.SizeOf<Native.ImageInfo>()));
        byte[] pixels = Native.ReadBytes((byte[]? buffer, nuint capacity, out nuint required) =>
            Native.uareu_image_get_pixels(image, buffer, capacity, out required));
        if ((ulong)pixels.Length != (ulong)info.StrideBytes * info.Height)
            throw new InvalidOperationException("Native pixel layout does not match image information.");
        Console.WriteLine($"{info.Width} x {info.Height}, {info.BitsPerPixel} bpp, {pixels.Length} bytes");
        Console.WriteLine(Native.ReadUtf8((byte[]? buffer, nuint capacity, out nuint required) =>
            Native.uareu_image_get_report_json(image, buffer, capacity, out required)));
    }
}
