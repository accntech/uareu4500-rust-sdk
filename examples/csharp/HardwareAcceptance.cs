// SPDX-License-Identifier: GPL-3.0-or-later
using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;

// Opt-in operator harness. Call markers are NOT proof of in-flight USB overlap.
internal static class HardwareAcceptance
{
    private static readonly object OutputLock = new();
    private static string Classify(bool interruption, bool disconnected, bool qualified, bool failed)
        => failed ? "failed" : interruption ? disconnected && qualified ? "passed" : disconnected || !qualified ? "inconclusive" : "failed" : "passed";
    private static bool ExpectedRaceFailure(string action, uint status, string kind)
        => action == "wait" && status == Native.Cancelled && kind == "cancelled";
    private static void Emit(string kind, string test, object details)
    {
        lock (OutputLock)
        {
            Console.WriteLine(JsonSerializer.Serialize(new { schema_version = 1, kind, test, utc = DateTimeOffset.UtcNow, ticks = Stopwatch.GetTimestamp(), tick_frequency = Stopwatch.Frequency, clock = "Stopwatch/QPC-on-Windows", details }));
            Console.Out.Flush();
        }
    }
    internal static int RunSelfTests()
    {
        static void Equal(string actual, string expected) { if (actual != expected) throw new InvalidOperationException($"Expected {expected}, received {actual}"); }
        Equal(Classify(false, false, false, false), "passed");
        Equal(Classify(true, true, true, false), "passed");
        Equal(Classify(true, true, false, false), "inconclusive");
        Equal(Classify(true, false, false, false), "inconclusive");
        Equal(Classify(true, false, true, false), "failed");
        Equal(Classify(false, false, false, true), "failed");
        if (!ExpectedRaceFailure("wait", Native.Cancelled, "cancelled") ||
            ExpectedRaceFailure("cancel", 2, "invalid_handle") || ExpectedRaceFailure("wait", 2, "invalid_handle") ||
            ExpectedRaceFailure("close", 2, "invalid_handle") || ExpectedRaceFailure("cancel", Native.Cancelled, "cancelled"))
            throw new InvalidOperationException("Race failure classification masked a retained-handle failure.");
        if (Overlap(100, 200, 120, 150, 180) == false || Overlap(100, 200, 120, 210, 220) || Overlap(100, 200, 90, 150, 180))
            throw new InvalidOperationException("Trace timing qualification failed.");
        Console.WriteLine("Hardware acceptance classification, overlap/watchdog policies and action-specific race failures verified without native calls.");
        return 0;
    }
    private static bool Overlap(long callStart, long callEnd, long transferStart, long disconnect, long transferEnd)
        => callStart <= transferStart && transferStart < disconnect && disconnect <= transferEnd && transferEnd <= callEnd;
    private static bool QualifiedTrace(string path, string test, long start, long end)
    {
        if (path == "-" || !File.Exists(path)) return false;
        using JsonDocument document = JsonDocument.Parse(File.ReadAllBytes(path));
        JsonElement root = document.RootElement;
        if (root.GetProperty("clock").GetString() != "windows_qpc" || root.GetProperty("tick_frequency").GetInt64() != Stopwatch.Frequency || root.GetProperty("test").GetString() != test)
            throw new ArgumentException("Trace clock, frequency or case identity mismatch.");
        string rawPath = Path.GetFullPath(root.GetProperty("raw_trace_path").GetString()!, Path.GetDirectoryName(Path.GetFullPath(path))!);
        string actualHash = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(rawPath)));
        if (actualHash != root.GetProperty("raw_trace_sha256").GetString()) throw new ArgumentException("Trace source hash mismatch.");
        return Overlap(start, end, root.GetProperty("transfer_start_ticks").GetInt64(), root.GetProperty("disconnect_ticks").GetInt64(), root.GetProperty("transfer_end_ticks").GetInt64());
    }
    private static ulong Open(bool persistent)
    {
        Native.Check(Native.uareu_reader_list_create(out ulong list));
        try
        {
            Native.Check(Native.uareu_reader_list_count(list, out uint count));
            if (count != 1) throw new InvalidOperationException("Exactly one supported reader is required; select an isolated test setup.");
            string identity = Native.ReadUtf8((byte[]? bytes, nuint capacity, out nuint required) => Native.uareu_reader_list_get_json(list, 0, bytes, capacity, out required));
            Emit("reader", "identity", JsonSerializer.Deserialize<JsonElement>(identity));
            Native.Check(Native.uareu_reader_open_ex(list, 0, persistent ? 1u : 0u, out ulong reader));
            return reader;
        }
        finally { Native.Check(Native.uareu_reader_list_free(list)); }
    }
    private static void CheckImage(ulong image)
    {
        try
        {
            Native.Check(Native.uareu_image_get_info(image, out Native.ImageInfo info, (nuint)System.Runtime.InteropServices.Marshal.SizeOf<Native.ImageInfo>()));
            byte[] pixels = Native.ReadBytes((byte[]? bytes, nuint capacity, out nuint required) => Native.uareu_image_get_pixels(image, bytes, capacity, out required));
            if (info.Width == 0 || info.Height == 0 || pixels.Length != checked((int)(info.Width * info.Height))) throw new InvalidOperationException("Invalid image geometry.");
            Emit("image", "capture", new { width = info.Width, height = info.Height, sha256 = Convert.ToHexStringLower(SHA256.HashData(pixels)) });
        }
        finally { Native.Check(Native.uareu_image_free(image)); }
    }
    private static string Status(ulong reader) => Native.ReadUtf8((byte[]? bytes, nuint capacity, out nuint required) => Native.uareu_reader_get_status_json(reader, bytes, capacity, out required));
    private static void Capture(ulong reader, string test)
    {
        Console.Error.WriteLine("Place a finger after the phase-2 prompt, hold still until the image result, then lift it.");
        Native.CaptureCallback callback = (phase, _) => Emit("phase", test, new { phase });
        Native.Check(Native.uareu_capture_with_callback(reader, 30_000, 1, callback, 0, out ulong image));
        CheckImage(image);
        GC.KeepAlive(callback);
    }
    private static async Task AsyncCapture(ulong reader, bool race, int order)
    {
        Native.Check(Native.uareu_capture_start(reader, 30_000, 1, out ulong task));
        try
        {
            async Task Wait()
            {
                await Task.Yield();
                uint completed;
                do
                {
                    uint status = Native.uareu_task_wait(task, 5000, out ulong image, out completed);
                    if (status == Native.Ok && image != 0) CheckImage(image);
                    else if (status != Native.Ok) Native.Check(status);
                } while (completed == 0);
            }
            if (!race) await Wait();
            else
            {
                (string Name, Func<Task> Run)[] actions = [
                    ("cancel", () => Task.Run(() => Native.Check(Native.uareu_task_cancel(task)))),
                    ("close", () => Task.Run(() => Native.Check(Native.uareu_reader_close(reader)))),
                    ("wait", Wait)
                ];
                async Task RunAction(string action, Func<Task> run)
                {
                    try { await run(); }
                    catch (Native.NativeException error) when (ExpectedRaceFailure(action, error.Status, error.Report.Kind)) { }
                }
                // The task handle remains owned until all three actions finish. An
                // invalid task handle in cancel/wait is a real lifecycle failure.
                var pending = Enumerable.Range(0, 3).Select(i => {
                    var action = actions[(i + order) % 3];
                    return Task.Run(() => RunAction(action.Name, action.Run));
                }).ToArray();
                await Task.WhenAll(pending);
            }
        }
        finally { Native.Check(Native.uareu_task_free(task)); }
    }
    private static void Continuous(ulong reader, string test)
    {
        int images = 0;
        bool removalRequested = false;
        string? callbackFailure = null;
        Native.CaptureCallback phase = (value, _) => { if (value == 3) removalRequested = true; Emit("phase", test, new { phase = value }); };
        Native.ImageCallback image = (handle, _) =>
        {
            try
            {
                if (images > 0 && !removalRequested) callbackFailure = "Second image delivered without the removal phase.";
                removalRequested = false;
                images++;
                CheckImage(handle);
                Console.Error.WriteLine(images == 1 ? "Keep the finger in place for five seconds, then lift and replace it." : "Continuous placement sequence complete.");
                return callbackFailure != null || images == 2 ? 1u : 0u;
            }
            catch (Exception error) { callbackFailure = error.Message; return 1; }
        };
        Native.Check(Native.uareu_capture_continuous(reader, 30_000, 1, image, phase, 0));
        GC.KeepAlive(image); GC.KeepAlive(phase);
        if (callbackFailure != null || images != 2) throw new InvalidOperationException(callbackFailure ?? "Continuous capture did not deliver two placements.");
    }
    internal static async Task<int> Run(string[] args)
    {
        if (args.Length != 3) throw new ArgumentException("--hardware-acceptance CASE ITERATIONS TRACE_JSON_OR_DASH");
        string test = args[0];
        if (!int.TryParse(args[1], out int iterations) || iterations < 1 || iterations > 100) throw new ArgumentException("Iterations must be 1..100.");
        bool interruption = test.StartsWith("unplug-", StringComparison.Ordinal);
        bool disconnected = false;
        bool failed = false;
        long start = 0, end = 0;
        ulong reader = Open(test != "placements-default");
        try
        {
            Emit("begin", test, new { iterations, os = Environment.OSVersion.ToString(), runtime = Environment.Version.ToString(), pid = Environment.ProcessId });
            start = Stopwatch.GetTimestamp();
            try
            {
                switch (test)
                {
                    case "placements-default":
                    case "placements-retained":
                        for (int i = 0; i < iterations; i++) { Capture(reader, test); Console.Error.WriteLine("Lift your finger and press Enter before the next scan."); if (Console.ReadLine() == null) throw new InvalidOperationException("Operator removal acknowledgement missing."); }
                        break;
                    case "prolonged":
                        var clock = Stopwatch.StartNew();
                        while (clock.Elapsed < TimeSpan.FromMinutes(30)) { Capture(reader, test); Console.Error.WriteLine("Lift finger and press Enter for the next placement."); if (Console.ReadLine() == null) throw new InvalidOperationException("Operator acknowledgement missing."); }
                        break;
                    case "continuous": Continuous(reader, test); break;
                    case "race": await AsyncCapture(reader, true, iterations % 3); reader = 0; break;
                    case "unplug-capture": Console.Error.WriteLine("Unplug during the active acquisition; preserve a qualifying external USB trace."); Capture(reader, test); break;
                    case "unplug-probe":
                        Console.Error.WriteLine("Unplug during the probe; call markers alone will not qualify overlap.");
                        Native.Check(Native.uareu_reader_probe(reader, out ulong data)); Native.Check(Native.uareu_data_free(data)); break;
                    case "unplug-recovery": Console.Error.WriteLine("Unplug during recovery; preserve a qualifying external USB trace."); Native.Check(Native.uareu_reader_recover(reader)); break;
                    case "unplug-async": Console.Error.WriteLine("Unplug during async acquisition; preserve a qualifying external USB trace."); await AsyncCapture(reader, false, 0); break;
                    default: throw new ArgumentException("Unknown acceptance case.");
                }
            }
            catch (Native.NativeException error) when (interruption && (error.Report.Kind == "disconnected" || error.Report.Cleanup?.Kind == "disconnected"))
            {
                disconnected = true;
                Emit("disconnect", test, new { error.Status, error.Report.Kind });
            }
            finally { end = Stopwatch.GetTimestamp(); }
            if (reader != 0)
            {
                string status = Status(reader);
                Emit("status", test, JsonSerializer.Deserialize<JsonElement>(status));
                if (disconnected)
                {
                    using JsonDocument document = JsonDocument.Parse(status);
                    if (document.RootElement.GetProperty("interface_claimed").GetBoolean()) throw new InvalidOperationException("Disconnected reader retained a claimed interface.");
                }
            }
        }
        catch (Exception error) { failed = true; Emit("error", test, new { type = error.GetType().Name, message = error.Message }); }
        finally
        {
            if (reader != 0) { try { Native.Check(Native.uareu_reader_close(reader)); } catch (Exception error) { failed = true; Emit("cleanup_error", test, new { message = error.Message }); } }
        }
        bool qualified = false;
        if (interruption && !failed)
        {
            Console.Error.WriteLine("Reconnect the reader and press Enter; a fresh reader handle must capture successfully.");
            if (Console.ReadLine() == null) failed = true;
            else
            {
                ulong fresh = 0;
                try { fresh = Open(true); Capture(fresh, "reconnect"); qualified = QualifiedTrace(args[2], test, start, end); }
                catch (Exception error) { failed = true; Emit("reconnect_error", test, new { message = error.Message }); }
                finally { if (fresh != 0) { try { Native.Check(Native.uareu_reader_close(fresh)); } catch { failed = true; } } }
            }
        }
        string result = Classify(interruption, disconnected, qualified, failed);
        Emit("result", test, new { status = result, disconnected, transfer_overlap_qualified = qualified, call_start_ticks = start, call_end_ticks = end, limitation = interruption && !qualified ? "USB overlap unproven; native call brackets do not qualify a phase-interruption case." : null });
        return result == "failed" ? 1 : result == "inconclusive" ? 3 : 0;
    }
}
