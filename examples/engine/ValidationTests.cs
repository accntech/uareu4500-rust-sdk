// SPDX-License-Identifier: GPL-3.0-or-later
using System.Formats.Cbor;
using System.Text.Json;
using SourceAFIS;

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
        byte[] cache = Record(31, 31, 0);
        FingerprintTemplate Parse(JsonElement value) => new(value.EnumerateArray().Select(b => b.GetByte()).ToArray());
        using JsonDocument comparison = JsonDocument.Parse(JsonSerializer.Serialize(new { probe = cache.Select(b => (int)b), candidate = cache.Select(b => (int)b), additional_candidates = new[] { cache.Select(b => (int)b).ToArray(), cache.Select(b => (int)b).ToArray() } }));
        using JsonDocument compared = JsonDocument.Parse(JsonSerializer.Serialize(ComparisonRequest.Run(comparison.RootElement, Parse)));
        if (compared.RootElement.GetProperty("additional_scores").GetArrayLength() != 2 ||
            compared.RootElement.GetProperty("additional_scores")[0].GetDouble() != compared.RootElement.GetProperty("score").GetDouble())
            throw new InvalidOperationException("Batched scores differ from the first identical comparison.");
        using JsonDocument legacy = JsonDocument.Parse(JsonSerializer.Serialize(new { probe = cache.Select(b => (int)b), candidate = cache.Select(b => (int)b) }));
        using JsonDocument legacyResponse = JsonDocument.Parse(JsonSerializer.Serialize(ComparisonRequest.Run(legacy.RootElement, Parse)));
        if (legacyResponse.RootElement.TryGetProperty("additional_scores", out _)) throw new InvalidOperationException("Legacy response changed.");
        using JsonDocument malformedBatch = JsonDocument.Parse(JsonSerializer.Serialize(new { probe = cache.Select(b => (int)b), candidate = cache.Select(b => (int)b), additional_candidates = "invalid" }));
        Rejected(() => ComparisonRequest.Run(malformedBatch.RootElement, Parse));
        Console.WriteLine("Native template validation and additive batch comparison verified.");
    }
}
