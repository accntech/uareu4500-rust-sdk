// SPDX-License-Identifier: GPL-3.0-or-later
using System.Runtime.InteropServices;
using System.Text;

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
        byte[] bytes = new byte[checked((int)required)];
        Check(copy(bytes, (nuint)bytes.Length, out nuint actual));
        if (actual != required)
            throw new InvalidOperationException("Native buffer size changed unexpectedly.");
        return bytes;
    }

    internal static string ReadUtf8(CopyBytes copy)
    {
        byte[] bytes = ReadBytes(copy);
        if (bytes.Length == 0 || bytes[^1] != 0)
            throw new InvalidOperationException("Native UTF-8 output has no terminator.");
        return new UTF8Encoding(false, true).GetString(bytes, 0, bytes.Length - 1);
    }

    internal static void Check(uint status)
    {
        if (status == Ok)
            return;
        // The DLL error is thread-local: retrieve it before leaving this thread.
        // Do not call Check recursively when retrieving an error.
        uint probe = uareu_last_error(null, 0, out nuint required);
        string message = "Native call failed.";
        if (probe == BufferTooSmall && required <= 1024 * 1024)
        {
            byte[] bytes = new byte[(int)required];
            if (uareu_last_error(bytes, required, out _) == Ok)
                message = Encoding.UTF8.GetString(bytes, 0, bytes.Length - 1);
        }
        throw new InvalidOperationException($"U.are.U status {status}: {message}");
    }
}
