using System.Text;
using System.Text.Json.Nodes;
using Nanshe.ObjectModel;
using Nanshe.Vba;

// The desktop exchanges bounded JSON lines over private stdin/stdout, never a listening port.
try
{
    if (args.Length == 2 && args[0] == "transpile")
    {
        Console.Write(Transpiler.Transpile(VbaModule.FromFile(args[1])).VbSource);
        return 0;
    }
    if (args.Length != 1 || args[0] != "bridge")
    {
        Console.Error.WriteLine("Usage: Nanshe.Vba bridge | transpile module.bas");
        return 2;
    }
    var request = JsonNode.Parse(Wire.Read()) ?? throw new InvalidDataException("Missing run request.");
    string source = (string?)request["source"] ?? "";
    if (Encoding.UTF8.GetByteCount(source) > 128 * 1024) throw new InvalidDataException("Source exceeds 128 KiB.");
    string macro = (string?)request["macro"] ?? "Main";
    var result = MacroCompiler.Compile([new("Module1", VbaModuleKind.Standard, source)]);
    if (!result.Success) throw new InvalidDataException(string.Join("\n", result.Diagnostics));
    using var project = result.Project!;
    VbaHost.Output = text => Wire.Send(new() { ["type"] = "output", ["text"] = text + "\n" });
    project.Run(new Application(new Bridge()), macro);
    Wire.Send(new() { ["type"] = "done" });
    return 0;
}
catch (Exception e)
{
    Wire.Send(new() { ["type"] = "error", ["message"] = e.Message.Length > 8000 ? e.Message[..8000] : e.Message });
    return 1;
}

static class Wire
{
    public static string Read()
    {
        var text = new StringBuilder();
        for (int i = 0; i < 512 * 1024; i++)
        {
            int c = Console.In.Read();
            if (c == '\n') return text.ToString();
            if (c < 0) throw new EndOfStreamException("Host disconnected.");
            text.Append((char)c);
        }
        throw new InvalidDataException("Protocol frame too large.");
    }
    public static void Send(JsonObject value) => Console.WriteLine(value.ToJsonString());
}

sealed class Bridge : IEngine
{
    int calls;
    public JsonNode? Execute(string command, JsonObject parameters)
    {
        if (++calls > 1000) throw new InvalidOperationException("Macro exceeded 1,000 engine calls.");
        Wire.Send(new() { ["type"] = "command", ["command"] = command, ["params"] = parameters });
        var reply = JsonNode.Parse(Wire.Read()) ?? throw new InvalidDataException("Missing host reply.");
        if (reply["error"] is { } error) throw new InvalidOperationException(error.ToString());
        return reply["result"]?.DeepClone();
    }
}
