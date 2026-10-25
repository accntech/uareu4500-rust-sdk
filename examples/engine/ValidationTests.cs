// SPDX-License-Identifier: GPL-3.0-or-later
using System.Formats.Cbor;
using System.Text.Json;

internal static class ValidationTests
{
    internal static void Run()
    {
        static void Rejected(Action action)
        {
            try { action(); }
            catch (ArgumentException) { return; }
            throw new InvalidOperationException("Malformed native cache was accepted.");
        }
        byte[] Record(int x, int y, double angle)
        {
            using JsonDocument json = JsonDocument.Parse(JsonSerializer.Serialize(new {
                width = 32, height = 32,
                minutiae = new[] { new { x, y, direction_clockwise = angle, kind = 1 } }
            }));
            return NativeTemplate.Create(json.RootElement);
        }
        _ = NativeTemplate.Read(Record(31, 31, 0));
        Rejected(() => Record(32, 0, 0));
        Rejected(() => Record(-1, 0, 0));
        Rejected(() => Record(0, 32, 0));
        Rejected(() => Record(0, 0, -0.1));
        Rejected(() => Record(0, 0, 7));
        var writer = new CborWriter();
        writer.WriteStartMap(2);
        writer.WriteTextString("width"); writer.WriteInt32(32);
        writer.WriteTextString("height"); writer.WriteInt32(32);
        writer.WriteEndMap();
        Rejected(() => NativeTemplate.Read(writer.Encode()));
        Console.WriteLine("Native template bounds, required fields, and invalid direction checks verified.");
    }
}
