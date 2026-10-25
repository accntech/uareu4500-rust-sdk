// SPDX-License-Identifier: GPL-3.0-or-later
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

internal static class Native
{
    internal const uint Ok = 0, BufferTooSmall = 3, Cancelled = 5;
    private const string Dll = "uareu";

    [StructLayout(LayoutKind.Sequential)]
    internal struct ImageInfo
    {
        public uint Width, Height, StrideBytes, BitsPerPixel;
    }

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_abi_version();
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_last_error([Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_last_error_json([Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_list_get_capabilities_json(ulong list, uint index, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_get_capabilities_json(ulong reader, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_get_status_json(ulong reader, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_probe(ulong reader, out ulong observation);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_recover(ulong reader);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_list_create(out ulong list);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_list_count(ulong list, out uint count);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_list_get_json(ulong list, uint index, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_list_free(ulong list);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_open(ulong list, uint index, out ulong reader);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_open_ex(ulong list, uint index, uint flags, out ulong reader);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_capture_start(ulong reader, uint timeoutMs, uint rotate180, out ulong task);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_task_wait(ulong task, uint timeoutMs, out ulong image, out uint completed);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_task_cancel(ulong task);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_task_free(ulong task);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_task_get_status_json(ulong task, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate uint ImageCallback(ulong image, nint context);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_capture_continuous(ulong reader, uint timeoutMs, uint rotate180, ImageCallback image, CaptureCallback phase, nint context);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_assess(ulong image, byte[] config, nuint length, out ulong report);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_enhance(ulong image, out ulong output);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_data_get_json(ulong data, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_data_free(ulong data);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_data_from_json(byte[] input, nuint length, out ulong output);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_from_pixels(uint width, uint height, byte[] pixels, nuint length, byte[]? metadata, nuint metadataLength, out ulong output);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_reader_close(ulong reader);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_capture(ulong reader, uint timeoutMs, uint rotate180, out ulong image);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void CaptureCallback(uint phase, nint context);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_capture_with_callback(ulong reader, uint timeoutMs, uint rotate180,
        CaptureCallback callback, nint context, out ulong image);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_cancel(ulong reader);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_get_info(ulong image, out ImageInfo info, nuint infoSize);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_get_pixels(ulong image, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_get_report_json(ulong image, [Out] byte[]? buffer, nuint capacity, out nuint required);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_load_png([In] byte[] path, nuint pathLength, out ulong image);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_save_png(ulong image, [In] byte[] path, nuint pathLength);
    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, ExactSpelling = true)]
    internal static extern uint uareu_image_free(ulong image);

    internal delegate uint CopyBytes(byte[]? buffer, nuint capacity, out nuint required);

    internal static byte[] ReadBytes(CopyBytes copy)
    {
        uint status = copy(null, 0, out nuint required);
        if (status != BufferTooSmall)
            Check(status);
        for (int attempt = 0; attempt < 8; attempt++)
        {
            byte[] bytes = new byte[checked((int)required)];
            status = copy(bytes, (nuint)bytes.Length, out nuint actual);
            if (status == BufferTooSmall) { required = actual; continue; }
            Check(status);
            if (actual > (nuint)bytes.Length)
                throw new InvalidOperationException("Native buffer size exceeds capacity.");
            return bytes[..checked((int)actual)];
        }
        throw new InvalidOperationException("Native snapshot changed too frequently; retry the query.");
    }

    internal static string ReadUtf8(CopyBytes copy)
    {
        byte[] bytes = ReadBytes(copy);
        if (bytes.Length == 0 || bytes[^1] != 0)
            throw new InvalidOperationException("Native UTF-8 output has no terminator.");
        return new UTF8Encoding(false, true).GetString(bytes, 0, bytes.Length - 1);
    }

    internal sealed record FailureDetails(string Kind, string? Stage, string Message, FailureDetails? Cleanup);
    internal sealed record ErrorReport(uint SchemaVersion, uint Status, string Kind, string? Stage, string Message, FailureDetails? Cleanup = null);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };
    internal sealed class NativeException(uint status, ErrorReport report)
        : Exception($"U.are.U status {status}, kind {report.Kind}, stage {report.Stage ?? "api"}: {report.Message}")
    {
        internal uint Status { get; } = status;
        internal ErrorReport Report { get; } = report;
    }
    // Retrieve before leaving the failing thread. Getter errors never recurse into Check.
    internal static ErrorReport ReadError()
    {
        uint probe = uareu_last_error_json(null, 0, out nuint required);
        if (probe != BufferTooSmall || required == 0 || required > 1024 * 1024)
            throw new InvalidOperationException("Cannot retrieve native error details.");
        byte[] bytes = new byte[(int)required];
        if (uareu_last_error_json(bytes, required, out nuint actual) != Ok || actual != required)
            throw new InvalidOperationException("Cannot copy native error details.");
        return JsonSerializer.Deserialize<ErrorReport>(bytes.AsSpan(0, bytes.Length - 1), JsonOptions)
            ?? throw new InvalidOperationException("Empty native error report.");
    }
    internal static void Check(uint status)
    {
        if (status != Ok) throw new NativeException(status, ReadError());
    }

    internal static void VerifySnapshotBuffers()
    {
        int calls = 0;
        byte[] bytes = ReadBytes((byte[]? buffer, nuint capacity, out nuint required) =>
        {
            calls++;
            required = calls == 1 ? 2u : calls == 2 ? 4u : 3u;
            if (capacity < required) return BufferTooSmall;
            for (int index = 0; index < 3; index++) buffer![index] = (byte)(index + 1);
            return Ok;
        });
        if (calls != 3 || !bytes.SequenceEqual(new byte[] { 1, 2, 3 }))
            throw new InvalidOperationException("Growing/shrinking snapshot buffers were not handled correctly.");
        uint status = uareu_capture(0, 30_000, 1, out ulong image);
        ErrorReport report = ReadError();
        if (status != 2 || image != 0 || report.Status != 2 || report.Kind != "invalid_handle" || report.Stage != null)
            throw new InvalidOperationException("Structured native errors did not retain legacy status and kind.");
    }
}
