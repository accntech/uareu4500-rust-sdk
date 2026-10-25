// SPDX-License-Identifier: GPL-3.0-or-later
using System.Runtime.InteropServices;
using System.Text;

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
            bool capture = args.Length is 2 or 3 && args[0] == "--capture";
            if (!listOnly && !cancelTest && !capture)
            {
                Console.Error.WriteLine("Usage: --list | --inspect image.png | --capture new.png [reader-index] | --cancel-test");
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
                Native.Check(Native.uareu_cancel(reader)); // Idle cancellation is harmless.
                ConsoleCancelEventHandler cancelHandler = (_, signal) =>
                {
                    signal.Cancel = true;
                    Native.uareu_cancel(reader);
                };
                Console.CancelKeyPress += cancelHandler;
                try
                {
                    Task<(uint Status, ulong Image, string? Error)> worker = Task.Run(() =>
                    {
                        Native.CaptureCallback progress = (phase, _) =>
                        {
                            // Managed exceptions must never cross a native callback.
                            try
                            {
                                Console.WriteLine(phase switch
                                {
                                    1 => "Initializing reader...",
                                    2 => "Place your finger flat on the reader and hold it until saved (Ctrl-C cancels).",
                                    3 => "Lift your finger from the reader.",
                                    4 => "Capturing; keep your finger still...",
                                    _ => $"Capture phase {phase}"
                                });
                            }
                            catch { /* Console output failure must not unwind into Rust. */ }
                        };
                        uint status = cancelTest
                            ? Native.uareu_capture(reader, 30_000, 1, out ulong image)
                            : Native.uareu_capture_with_callback(reader, 30_000, 1, progress, 0, out image);
                        // Copy any error while still on the native-calling thread.
                        string? error = status == Native.Ok ? null : Native.ReadUtf8(Native.uareu_last_error);
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
                    if (result.Status == Native.Cancelled)
                    {
                        Console.WriteLine("Capture cancelled; reader cleanup completed.");
                        return 0;
                    }
                    if (result.Status != Native.Ok)
                        throw new InvalidOperationException($"Capture status {result.Status}: {result.Error}");
                    try
                    {
                        if (cancelTest)
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
