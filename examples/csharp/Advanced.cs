// SPDX-License-Identifier: GPL-3.0-or-later
using System.Globalization;
using System.Text;
using System.Text.Json;

internal static class Advanced
{
    internal static readonly HashSet<string> Commands = ["--async-test", "--capture-async", "--continuous", "--probe", "--recover", "--session-test"];
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
            default: throw new ArgumentException("Invalid advanced command arguments; see README for usage.");
        }
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
