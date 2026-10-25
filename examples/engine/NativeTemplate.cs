// SPDX-License-Identifier: GPL-3.0-or-later
// Native SourceAFIS cache structure documented at sourceafis.machinezoo.com/template.
using System.Formats.Cbor;
using System.Text.Json;
using SourceAFIS;

internal static class NativeTemplate
{
    internal static object Read(byte[] bytes)
    {
        var reader = new CborReader(bytes);
        int count = reader.ReadStartMap() ?? throw new ArgumentException("Indefinite map unsupported.");
        if (count > 32) throw new ArgumentException("Too many cache fields.");
        int width = 0, height = 0;
        int[] x = [], y = [];
        float[] directions = [];
        string types = "";
        var seen = new HashSet<string>();
        int[] Integers()
        {
            int n = reader.ReadStartArray() ?? throw new ArgumentException("Indefinite array unsupported.");
            if (n > 1000) throw new ArgumentException("Too many minutiae.");
            int[] values = new int[n];
            for (int i = 0; i < n; i++) values[i] = reader.ReadInt32();
            reader.ReadEndArray(); return values;
        }
        for (int i = 0; i < count; i++)
        {
            string key = reader.ReadTextString();
            if (!seen.Add(key)) throw new ArgumentException("Duplicate native cache field.");
            switch (key)
            {
                case "width": width = reader.ReadInt32(); break;
                case "height": height = reader.ReadInt32(); break;
                case "positionsX": x = Integers(); break;
                case "positionsY": y = Integers(); break;
                case "types": types = reader.ReadTextString(); break;
                case "directions":
                    int n = reader.ReadStartArray() ?? throw new ArgumentException("Indefinite directions unsupported.");
                    if (n > 1000) throw new ArgumentException("Too many minutiae.");
                    directions = new float[n];
                    for (int j = 0; j < n; j++) directions[j] = reader.PeekState() == CborReaderState.SinglePrecisionFloat ? reader.ReadSingle() : (float)reader.ReadDouble();
                    reader.ReadEndArray(); break;
                case "version":
                    string version = reader.ReadTextString();
                    if (version.Length == 0 || version.Length > 1000) throw new ArgumentException("Invalid version.");
                    break;
                default: throw new ArgumentException("Unknown native cache field.");
            }
        }
        reader.ReadEndMap();
        if (!seen.SetEquals(["width", "height", "positionsX", "positionsY", "directions", "types", "version"]) ||
            reader.BytesRemaining != 0 || x.Length != types.Length || y.Length != types.Length || directions.Length != types.Length)
            throw new ArgumentException("Invalid native cache.");
        if (width <= 0 || height <= 0 || width > 10000 || height > 10000 ||
            x.Any(n => n < 0 || n >= width) || y.Any(n => n < 0 || n >= height) ||
            directions.Any(d => !float.IsFinite(d) || d < 0 || d >= MathF.Tau) || types.Any(t => t != 'E' && t != 'B'))
            throw new ArgumentException("Invalid native cache values.");
        _ = new FingerprintTemplate(bytes); // Only after bounded structural checks.
        return new { width, height, positions_x = x, positions_y = y, directions, types };
    }
    internal static byte[] Create(JsonElement record)
    {
        int width = record.GetProperty("width").GetInt32(), height = record.GetProperty("height").GetInt32();
        var minutiae = record.GetProperty("minutiae").EnumerateArray().ToArray();
        if (width <= 0 || height <= 0 || width > 10000 || height > 10000 || minutiae.Length > 255)
            throw new ArgumentException("Unsupported native geometry.");
        var writer = new CborWriter();
        writer.WriteStartMap(7);
        writer.WriteTextString("version"); writer.WriteTextString(FingerprintCompatibility.Version + "-net");
        writer.WriteTextString("width"); writer.WriteInt32(width);
        writer.WriteTextString("height"); writer.WriteInt32(height);
        foreach (var (name, key) in new[] { ("positionsX", "x"), ("positionsY", "y") })
        {
            writer.WriteTextString(name); writer.WriteStartArray(minutiae.Length);
            foreach (var m in minutiae) writer.WriteInt32(m.GetProperty(key).GetInt32());
            writer.WriteEndArray();
        }
        writer.WriteTextString("directions"); writer.WriteStartArray(minutiae.Length);
        foreach (var m in minutiae) writer.WriteSingle(m.GetProperty("direction_clockwise").GetSingle());
        writer.WriteEndArray();
        writer.WriteTextString("types"); writer.WriteTextString(new string(minutiae.Select(m => m.GetProperty("kind").GetInt32() switch { 1 => 'E', 2 => 'B', _ => throw new ArgumentException("Unknown minutia type cannot be imported into SourceAFIS.") }).ToArray()));
        writer.WriteEndMap();
        byte[] bytes = writer.Encode(); _ = Read(bytes); return bytes;
    }
}
