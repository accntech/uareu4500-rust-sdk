// SPDX-License-Identifier: GPL-3.0-or-later
// Optional engine process. Uses SourceAFIS, Apache-2.0, by Robert Vazan.
using SourceAFIS;
using System.Security.Cryptography;
using System.Text.Json;

try
{
    if (args is ["--self-test"]) { ValidationTests.Run(); return 0; }
    if (Console.IsInputRedirected == false) throw new ArgumentException("Supply one JSON request on stdin.");
    using Stream input = Console.OpenStandardInput();
    using var bounded = new MemoryStream();
    byte[] chunk = new byte[8192];
    int count;
    while ((count = input.Read(chunk)) > 0)
    {
        if (bounded.Length + count > 16 * 1024 * 1024) throw new ArgumentException("Request too large.");
        bounded.Write(chunk, 0, count);
    }
    using JsonDocument document = JsonDocument.Parse(bounded.ToArray(), new JsonDocumentOptions { MaxDepth = 32 });
    JsonElement request = document.RootElement;
    string op = request.GetProperty("operation").GetString() ?? "";
    byte[] Bytes(JsonElement value)
    {
        if (value.GetArrayLength() > 1024 * 1024) throw new ArgumentException("Template/image too large.");
        return value.EnumerateArray().Select(v => v.GetByte()).ToArray();
    }
    FingerprintTemplate Template(JsonElement value)
    {
        byte[] bytes = Bytes(value); _ = NativeTemplate.Read(bytes); return new(bytes);
    }
    object result;
    if (op == "extract")
    {
        int width = request.GetProperty("width").GetInt32();
        int height = request.GetProperty("height").GetInt32();
        double ppi = request.GetProperty("ppi").GetDouble();
        if (width < 16 || height < 16 || width > 2048 || height > 2048 ||
            !double.IsFinite(ppi) || ppi < 250 || ppi > 2000 || (double)width * height * Math.Pow(500 / ppi, 2) > 4_000_000)
            throw new ArgumentException("Unsupported image geometry/resolution.");
        byte[] pixels = Bytes(request.GetProperty("pixels"));
        if (pixels.Length != checked(width * height)) throw new ArgumentException("Pixel count mismatch.");
        var template = new FingerprintTemplate(new FingerprintImage(width, height, pixels, new FingerprintImageOptions { Dpi = ppi }));
        result = new { data = template.ToByteArray().Select(b => (int)b).ToArray() };
    }
    else if (op == "compare") result = ComparisonRequest.Run(request, Template);
    else if (op == "minutiae") result = NativeTemplate.Read(Bytes(request.GetProperty("template")));
    else if (op == "import_minutiae") result = new { data = NativeTemplate.Create(request.GetProperty("record")).Select(b => (int)b).ToArray() };
    else throw new ArgumentException("Unknown operation.");
    static string Hash(string path) => Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(path)));
    Console.WriteLine(JsonSerializer.Serialize(new {
        schema_version = 1, engine = "SourceAFIS", version = FingerprintCompatibility.Version,
        engine_sha256 = Hash(typeof(FingerprintTemplate).Assembly.Location),
        host_sha256 = Hash(typeof(Program).Assembly.Location), result
    }));
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(error.Message);
    return 1;
}

// Private additive request extension. Older helpers ignore additional_candidates;
// newer helpers retain the original single-candidate response when it is absent.
internal static class ComparisonRequest
{
    internal static object Run(JsonElement request, Func<JsonElement, FingerprintTemplate> template)
    {
        var matcher = new FingerprintMatcher(template(request.GetProperty("probe")));
        double Match(JsonElement candidate)
        {
            double value = matcher.Match(template(candidate));
            if (!double.IsFinite(value) || value < 0) throw new InvalidOperationException("Invalid engine score.");
            return value;
        }
        double score = Match(request.GetProperty("candidate"));
        if (!request.TryGetProperty("additional_candidates", out JsonElement additional)) return new { score };
        if (additional.ValueKind != JsonValueKind.Array || additional.GetArrayLength() > 999)
            throw new ArgumentException("At most 999 additional candidates are supported.");
        double[] scores = new double[additional.GetArrayLength()];
        int index = 0;
        foreach (JsonElement candidate in additional.EnumerateArray()) scores[index++] = Match(candidate);
        return new { score, additional_scores = scores };
    }
}
