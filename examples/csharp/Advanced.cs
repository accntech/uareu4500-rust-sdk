// SPDX-License-Identifier: GPL-3.0-or-later
using System.Globalization;
using System.Text;
using System.Text.Json;

internal static class Advanced
{
    internal static readonly HashSet<string> Commands = ["--advanced-test", "--recognition-test", "--session-test", "--async-test", "--continuous", "--capture-async", "--quality", "--enhance", "--extract", "--compare", "--enroll", "--identify", "--verify-enrollment", "--export-template", "--import-template", "--wsq-encode", "--wsq-decode", "--probe", "--recover", "--image-record-encode", "--image-record-decode"];
    private sealed class Handles : IDisposable
    {
        private readonly List<(ulong Value, bool Image)> items = [];
        internal ulong Image(ulong value) { items.Add((value, true)); return value; }
        internal ulong Data(ulong value) { items.Add((value, false)); return value; }
        public void Dispose() { foreach (var item in items.AsEnumerable().Reverse()) { if (item.Image) Native.uareu_image_free(item.Value); else Native.uareu_data_free(item.Value); } }
    }
    private static byte[] Json(object value) => JsonSerializer.SerializeToUtf8Bytes(value);
    private static string Data(ulong handle) => Native.ReadUtf8((byte[]? bytes, nuint capacity, out nuint required) => Native.uareu_data_get_json(handle, bytes, capacity, out required));
    private static string Status(ulong reader) => Native.ReadUtf8((byte[]? bytes, nuint capacity, out nuint required) => Native.uareu_reader_get_status_json(reader, bytes, capacity, out required));
    private static ulong Load(string path, Handles handles)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(path); Native.Check(Native.uareu_image_load_png(bytes, (nuint)bytes.Length, out ulong image)); return handles.Image(image);
    }
    private static ulong ImportData(string json, Handles handles)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(json); Native.Check(Native.uareu_data_from_json(bytes, (nuint)bytes.Length, out ulong data)); return handles.Data(data);
    }
    private static void Save(ulong image, string path)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(path); Native.Check(Native.uareu_image_save_png(image, bytes, (nuint)bytes.Length)); Console.WriteLine($"Saved: {path}");
    }
    private static void WriteNew(string path, byte[] bytes) { using var file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None); file.Write(bytes); }
    private static ulong Recognize(string executable, object request, Handles handles, double? ppi = null)
    {
        byte[] config = Json(new { executable = Path.GetFullPath(executable), timeout_ms = 30_000, ppi }); byte[] input = Json(request);
        Native.Check(Native.uareu_recognition_execute(config, (nuint)config.Length, input, (nuint)input.Length, out ulong output)); return handles.Data(output);
    }
    private static double Number(string value) => double.Parse(value, CultureInfo.InvariantCulture);
    private static void Require(bool condition, string message) { if (!condition) throw new InvalidOperationException(message); }
    private static ulong Open(bool persistent)
    {
        Native.Check(Native.uareu_reader_list_create(out ulong list));
        try { Native.Check(Native.uareu_reader_open_ex(list, 0, persistent ? 1u : 0u, out ulong reader)); return reader; }
        finally { Native.uareu_reader_list_free(list); }
    }
    internal static async Task<int> Run(string[] args)
    {
        using Handles handles = new();
        switch (args[0])
        {
            case "--advanced-test": OfflineTest(handles); return 0;
            case "--recognition-test" when args.Length == 2: RecognitionTest(args[1], handles); return 0;
            case "--session-test": SessionTest(); return 0;
            case "--probe":
            case "--recover":
            {
                ulong reader = Open(true);
                try
                {
                    if (args[0] == "--recover") Native.Check(Native.uareu_reader_recover(reader));
                    Native.Check(Native.uareu_reader_probe(reader, out ulong observation)); handles.Data(observation);
                    Console.WriteLine(Data(observation)); return 0;
                }
                finally { Native.uareu_reader_close(reader); }
            }
            case "--async-test": await AsyncCapture(null, true); return 0;
            case "--capture-async" when args.Length == 2: await AsyncCapture(args[1], false); return 0;
            case "--continuous" when args.Length == 2: Continuous(args[1]); return 0;
            case "--quality" when args.Length is 2 or 3:
            {
                ulong image = Load(args[1], handles);
                byte[] config = Json(new { engine_path = args.Length == 3 ? Path.GetFullPath(args[2]) : null as string });
                Native.Check(Native.uareu_image_assess(image, config, (nuint)config.Length, out ulong report)); handles.Data(report); Console.WriteLine(Data(report)); return 0;
            }
            case "--image-record-encode" when args.Length == 4:
            {
                byte[] config = File.ReadAllBytes(args[3]);
                Native.Check(Native.uareu_image_encode_record(Load(args[1], handles), config, (nuint)config.Length, out ulong encoded)); handles.Data(encoded);
                using JsonDocument record = JsonDocument.Parse(Data(encoded)); WriteNew(args[2], record.RootElement.EnumerateArray().Select(p => p.GetByte()).ToArray()); return 0;
            }
            case "--image-record-decode" when args.Length == 3:
            {
                ulong encoded = ImportData(JsonSerializer.Serialize(File.ReadAllBytes(args[1]).Select(b => (int)b)), handles);
                Native.Check(Native.uareu_image_decode_record(encoded, out ulong image)); handles.Image(image); Save(image, args[2]); return 0;
            }
            case "--enhance" when args.Length == 3:
            {
                Native.Check(Native.uareu_image_enhance(Load(args[1], handles), out ulong image)); handles.Image(image); Save(image, args[2]); return 0;
            }
            case "--extract" when args.Length is 4 or 5:
            {
                ulong template = Recognize(args[3], new { operation = "extract", image = Load(args[1], handles) }, handles, args.Length == 5 ? Number(args[4]) : null);
                WriteNew(args[2], Encoding.UTF8.GetBytes(Data(template))); return 0;
            }
            case "--compare" when args.Length == 5:
            case "--verify-enrollment" when args.Length == 5:
            {
                ulong probe = ImportData(File.ReadAllText(args[1]), handles), candidate = ImportData(File.ReadAllText(args[2]), handles);
                object request = args[0] == "--compare" ? new { operation = "compare", probe, candidate, threshold = Number(args[3]) } : new { operation = "verify_enrollment", probe, enrollment = candidate, threshold = Number(args[3]) };
                Console.WriteLine(Data(Recognize(args[4], request, handles))); return 0;
            }
            case "--enroll" when args.Length >= 6:
            {
                ulong[] samples = args.Skip(4).Select(path => ImportData(File.ReadAllText(path), handles)).ToArray();
                ulong enrollment = Recognize(args[3], new { operation = "enroll", samples, threshold = Number(args[2]) }, handles);
                WriteNew(args[1], Encoding.UTF8.GetBytes(Data(enrollment))); return 0;
            }
            case "--identify" when args.Length >= 5:
            {
                ulong probe = ImportData(File.ReadAllText(args[1]), handles);
                ulong[] candidates = args.Skip(4).Select(path => ImportData(File.ReadAllText(path), handles)).ToArray();
                Console.WriteLine(Data(Recognize(args[3], new { operation = "identify", probe, candidates, threshold = Number(args[2]), maximum = 10 }, handles))); return 0;
            }
            case "--export-template" when args.Length == 7:
            {
                ulong template = ImportData(File.ReadAllText(args[1]), handles);
                ulong encoded = Recognize(args[6], new { operation = "export_template", template, format = args[3], quality = byte.Parse(args[4]), impression = byte.Parse(args[5]) }, handles);
                using JsonDocument document = JsonDocument.Parse(Data(encoded)); WriteNew(args[2], document.RootElement.EnumerateArray().Select(p => p.GetByte()).ToArray()); return 0;
            }
            case "--import-template" when args.Length == 5:
            {
                ulong encoded = ImportData(JsonSerializer.Serialize(File.ReadAllBytes(args[1]).Select(b => (int)b)), handles);
                ulong template = Recognize(args[4], new { operation = "import_template", data = encoded, format = args[3] }, handles);
                WriteNew(args[2], Encoding.UTF8.GetBytes(Data(template))); return 0;
            }
            case "--wsq-encode" when args.Length is 5 or 6:
            case "--wsq-decode" when args.Length == 5:
            {
                byte[] config = Json(new { encoder = Path.GetFullPath(args[3]), decoder = Path.GetFullPath(args[4]), bitrate = args.Length == 6 ? Number(args[5]) : 0.75, timeout_ms = 30_000 });
                if (args[0] == "--wsq-encode")
                {
                    Native.Check(Native.uareu_image_encode_wsq(Load(args[1], handles), config, (nuint)config.Length, out ulong data)); handles.Data(data);
                    using JsonDocument document = JsonDocument.Parse(Data(data)); WriteNew(args[2], document.RootElement.EnumerateArray().Select(p => p.GetByte()).ToArray());
                }
                else
                {
                    ulong data = ImportData(JsonSerializer.Serialize(File.ReadAllBytes(args[1]).Select(b => (int)b)), handles);
                    Native.Check(Native.uareu_image_decode_wsq(data, config, (nuint)config.Length, out ulong image)); handles.Image(image); Save(image, args[2]);
                }
                return 0;
            }
            default: throw new ArgumentException("Invalid advanced command arguments; see README for usage.");
        }
    }
    private static void OfflineTest(Handles handles)
    {
        byte[] pixels = Enumerable.Range(0, 1024).Select(i => (byte)(200 + i % 50)).ToArray();
        Native.Check(Native.uareu_image_from_pixels(32, 32, pixels, (nuint)pixels.Length, null, 0, out ulong original)); handles.Image(original);
        Native.Check(Native.uareu_image_enhance(original, out ulong enhanced)); handles.Image(enhanced);
        byte[] copy = Native.ReadBytes((byte[]? buffer, nuint capacity, out nuint required) => Native.uareu_image_get_pixels(original, buffer, capacity, out required));
        Require(copy.SequenceEqual(pixels), "Enhancement changed original pixels.");
        byte[] config = Json(new { engine_path = "absent-engine" });
        Native.Check(Native.uareu_image_assess(original, config, (nuint)config.Length, out ulong report)); handles.Data(report);
        using JsonDocument result = JsonDocument.Parse(Data(report));
        Require(result.RootElement.GetProperty("metadata").GetProperty("resolution").GetProperty("status").GetString() == "unknown", "Resolution was guessed.");
        Require(result.RootElement.GetProperty("quality").GetProperty("status").GetString() == "unsupported_input", "Unknown resolution must prevent NFIQ invocation.");
        byte[] metadata = Json(new { resolution = new { status = "known", x_ppi = 500, y_ppi = 500, provenance = new { source = "caller_declared" } }, valid_region = (object?)null, processing = Array.Empty<string>() });
        Native.Check(Native.uareu_image_from_pixels(32, 32, pixels, (nuint)pixels.Length, metadata, (nuint)metadata.Length, out ulong declared)); handles.Image(declared);
        byte[] recordOptions = Json(new { capture_device_id = 0, acquisition_level = 0, scale_units = 1, scan_resolution_x = 500, scan_resolution_y = 500, finger_position = 0, quality = 0, impression = 0 });
        Native.Check(Native.uareu_image_encode_record(declared, recordOptions, (nuint)recordOptions.Length, out ulong encoded)); handles.Data(encoded);
        Native.Check(Native.uareu_image_decode_record(encoded, out ulong decoded)); handles.Image(decoded);
        byte[] roundTrip = Native.ReadBytes((byte[]? buffer, nuint capacity, out nuint required) => Native.uareu_image_get_pixels(decoded, buffer, capacity, out required));
        Require(roundTrip.SequenceEqual(pixels), "Raw ISO image-record conversion changed pixels.");
        Console.WriteLine("Managed enhancement, immutable reports, raw image import, and unknown-resolution gates verified.");
    }
    private static void RecognitionTest(string engine, Handles handles)
    {
        // Deterministic synthetic ridge field: no human fingerprint is used.
        byte[] Pixels(int brightness) => Enumerable.Range(0, 256 * 256).Select(i => {
            double x = i % 256 - 128, y = i / 256 - 128;
            double phase = Math.Sqrt(x*x + y*y) * 0.7 + Math.Atan2(y, x) * 3 + Math.Sin(x / 18);
            return (byte)Math.Clamp((Math.Cos(phase) > 0.15 ? 35 : 225) + brightness, 0, 255);
        }).ToArray();
        byte[] metadata = Json(new { resolution = new { status = "known", x_ppi = 500, y_ppi = 500, provenance = new { source = "caller_declared" } }, valid_region = (object?)null, processing = Array.Empty<string>() });
        ulong Extract(int brightness)
        {
            byte[] pixels = Pixels(brightness); Native.Check(Native.uareu_image_from_pixels(256, 256, pixels, (nuint)pixels.Length, metadata, (nuint)metadata.Length, out ulong image)); handles.Image(image);
            return Recognize(engine, new { operation = "extract", image }, handles);
        }
        ulong first = Extract(0), second = Extract(3);
        ulong comparison = Recognize(engine, new { operation = "compare", probe = first, candidate = second, threshold = 1.0 }, handles);
        using JsonDocument match = JsonDocument.Parse(Data(comparison));
        Require(match.RootElement.GetProperty("matched").GetBoolean(), "Synthetic ridge-field variants did not match at integration-test threshold 1.");
        ulong enrollment = Recognize(engine, new { operation = "enroll", samples = new[] { first, second }, threshold = 1.0 }, handles);
        ulong verification = Recognize(engine, new { operation = "verify_enrollment", probe = first, enrollment, threshold = 1.0 }, handles);
        using JsonDocument verified = JsonDocument.Parse(Data(verification)); Require(verified.RootElement.GetProperty("matched").GetBoolean(), "Enrollment verification failed.");
        ulong identified = Recognize(engine, new { operation = "identify", probe = first, candidates = new[] { second }, threshold = 1.0, maximum = 1 }, handles);
        using JsonDocument candidates = JsonDocument.Parse(Data(identified)); Require(candidates.RootElement.GetArrayLength() == 1, "Identification failed.");
        foreach (string format in new[] { "iso19794_2_2005", "ansi378_2004" })
        {
            ulong encoded = Recognize(engine, new { operation = "export_template", template = first, format, quality = 0, impression = 0 }, handles);
            ulong imported = Recognize(engine, new { operation = "import_template", data = encoded, format }, handles);
            ulong compared = Recognize(engine, new { operation = "compare", probe = first, candidate = imported, threshold = 1.0 }, handles);
            using JsonDocument result = JsonDocument.Parse(Data(compared)); Require(result.RootElement.GetProperty("matched").GetBoolean(), "Interchange round trip failed.");
        }
        Console.WriteLine("SourceAFIS extraction, comparison, enrollment, identification, and single-view ANSI/ISO conversion verified on synthetic data; no accuracy/FMR claim.");
    }
    private static void SessionTest()
    {
        ulong reader = Open(true);
        try
        {
            Native.Check(Native.uareu_reader_probe(reader, out ulong observation));
            try
            {
                using JsonDocument probe = JsonDocument.Parse(Data(observation));
                Require(probe.RootElement.GetProperty("connected_at_observation").GetBoolean(), "Probe did not observe USB response.");
                Require(probe.RootElement.GetProperty("hardware_ready").ValueKind == JsonValueKind.Null, "Probe guessed readiness.");
            }
            finally { Native.uareu_data_free(observation); }
            Native.Check(Native.uareu_reader_recover(reader));
            for (int round = 0; round < 2; round++)
            {
                Native.CaptureCallback phase = (state, _) => { if (state == 2) Native.uareu_cancel(reader); };
                uint status = Native.uareu_capture_with_callback(reader, 10_000, 1, phase, 0, out ulong image);
                Native.ErrorReport error = Native.ReadError();
                Require(status == Native.Cancelled && image == 0 && error.Kind == "cancelled", "Session cancellation failed.");
                using JsonDocument snapshot = JsonDocument.Parse(Status(reader)); Require(snapshot.RootElement.GetProperty("interface_claimed").GetBoolean(), "Persistent interface was released.");
            }
            Native.ImageCallback frame = (image, _) => { Native.uareu_image_free(image); return 1; };
            Native.CaptureCallback stop = (state, _) => { if (state == 2) Native.uareu_cancel(reader); };
            uint continuous = Native.uareu_capture_continuous(reader, 10_000, 1, frame, stop, 0);
            Require(continuous == Native.Cancelled && Native.ReadError().Kind == "cancelled", "Continuous cancellation lost its stop reason.");
            Console.WriteLine("Persistent interface rearming and continuous capture cancellation verified.");
        }
        finally { Native.uareu_reader_close(reader); }
    }
    private static async Task AsyncCapture(string? output, bool test)
    {
        ulong reader = Open(true), job = 0, image = 0;
        try
        {
            Native.Check(Native.uareu_capture_start(reader, 30_000, 1, out job));
            if (test)
            {
                uint duplicate = Native.uareu_capture(reader, 1, 1, out ulong ignored);
                Require(duplicate == 4 && ignored == 0, "Async start did not reserve reader.");
                Native.Check(Native.uareu_task_cancel(job));
            }
            else Console.WriteLine("Place your finger on the reader; Ctrl-C cancels the async job.");
            ConsoleCancelEventHandler cancel = (_, signal) => { signal.Cancel = true; Native.uareu_task_cancel(job); };
            Console.CancelKeyPress += cancel;
            try
            {
                while (true)
                {
                    uint status = Native.uareu_task_wait(job, 50, out image, out uint completed);
                    if (status != Native.Ok)
                    {
                        Native.ErrorReport error = Native.ReadError();
                        if (test && status == Native.Cancelled && completed == 1 && image == 0 && error.Kind == "cancelled") break;
                        throw new Native.NativeException(status, error);
                    }
                    if (completed == 1) { Require(!test, "Capture completed before test cancellation."); Save(image, output!); break; }
                    await Task.Delay(10);
                }
            }
            finally { Console.CancelKeyPress -= cancel; }
            if (test)
            {
                uint repeated = Native.uareu_task_wait(job, 0, out ulong duplicateImage, out uint duplicateCompleted);
                Require(repeated == 1 && duplicateImage == 0 && duplicateCompleted == 0, "Async completion was consumed twice.");
                Native.Check(Native.uareu_task_free(job)); job = 0;
                Native.Check(Native.uareu_capture_start(reader, 30_000, 1, out job));
                Native.Check(Native.uareu_reader_close(reader)); reader = 0;
                uint closed = Native.uareu_task_wait(job, 5_000, out image, out uint closedCompleted);
                Require(closed == Native.Cancelled && closedCompleted == 1 && image == 0, "Reader close did not cancel its pending job.");
                Console.WriteLine("Native async reservation, single completion, close cancellation, and result delivery verified.");
            }
        }
        finally { if (image != 0) Native.uareu_image_free(image); if (job != 0) Native.uareu_task_free(job); if (reader != 0) Native.uareu_reader_close(reader); }
    }
    private static void Continuous(string directory)
    {
        Directory.CreateDirectory(directory); ulong reader = Open(true); Exception? failure = null;
        Native.ImageCallback frame = (image, _) => {
            try { Save(image, Path.Combine(directory, "uareu-" + Guid.NewGuid().ToString("N") + ".png")); return 0; }
            catch (Exception error) { failure = error; return 1; }
            finally { Native.uareu_image_free(image); }
        };
        Native.CaptureCallback phase = (state, _) => { try { Console.WriteLine(state == 2 ? "Place your finger." : state == 3 ? "Lift your finger." : state == 4 ? "Capturing; hold still." : "Initializing..."); } catch { } };
        ConsoleCancelEventHandler cancel = (_, signal) => { signal.Cancel = true; Native.uareu_cancel(reader); };
        Console.CancelKeyPress += cancel;
        try { uint status = Native.uareu_capture_continuous(reader, 30_000, 1, frame, phase, 0); if (status != Native.Cancelled) Native.Check(status); if (failure != null) throw failure; }
        finally { Console.CancelKeyPress -= cancel; Native.uareu_reader_close(reader); }
    }
}
