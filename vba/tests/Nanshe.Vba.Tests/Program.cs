using System.Text.Json.Nodes;
using Nanshe.ObjectModel;
using Nanshe.Vba;

int passed = 0;
void Check(bool condition, string message)
{
    if (!condition) throw new Exception(message);
    passed++;
}
MacroCompiler.Result Compile(string body) => MacroCompiler.Compile([new("Module1", VbaModuleKind.Standard, body)]);
void Run(string body, FakeEngine engine)
{
    var result = Compile(body);
    if (!result.Success) throw new Exception(string.Join("\n", result.Diagnostics));
    using var project = result.Project!;
    project.Run(new Application(engine), "Main");
}

var fake = new FakeEngine();
Run("""
Option Explicit
Sub Main()
    Dim ws As Excel.Worksheet
    Set ws = Worksheets("Sheet1")
    ws.Range("A1").Value = "0012"
    ws.Cells(2, 1).Value = "=not a formula"
    ws.Range("B1:B2").Formula = "=1+2"
    ws.Range("A1:B2").Font.Bold = True
    ws.Range("A1:B2").NumberFormat = "0.00"
End Sub
""", fake);
Check(fake.Calls.Any(x => x.Command == "range.setValues" && (string?)x.Params["values"]?[0]?[0] == "'0012"), "Value preserves numeric-looking text");
Check(fake.Calls.Any(x => (string?)x.Params["values"]?[0]?[0] == "'=not a formula"), "Value cannot inject formulas");
Check(fake.Calls.Any(x => x.Command == "home.bold" && (bool?)x.Params["on"] == true), "Font uses explicit on rather than toggling");
Check(fake.Calls.Any(x => x.Command == "home.numberFormat" && (string?)x.Params["code"] == "0.00"), "NumberFormat maps to engine code");
Check(fake.Calls.Any(x => (string?)x.Params["range"] == "B1:B2" && x.Params["values"]?.AsArray().Count == 2), "Scalar formula fills the requested rectangle");

string output = "";
VbaHost.Output = s => output += s;
Run("""
Sub Main()
    Dim n As Long
    n = 1
    AddOne n
    If n = 2 Then Debug.Print "Привет"
    Debug.Print Range("A1").Value
End Sub
Sub AddOne(n As Long)
    n = n + 1
End Sub
""", fake);
Check(output == "Привет42", "ByRef, inline If, Unicode and engine reads work");
foreach (string malicious in new[] {
    "Shell \"anything\"", "CreateObject(\"WScript.Shell\")", "System.IO.File.Delete(\"anything\")",
    "Debug.Print Environ(\"HOME\")", "Debug.Print Application.GetType()", "CallByName Application, \"Call\", 1",
})
    Check(!Compile($"Sub Main()\n{malicious}\nEnd Sub").Success, $"Blocked: {malicious}");
Check(!Compile("Declare Sub unsafe Lib \"kernel32\" ()").Success, "Native declarations refused");
Check(!Compile("Sub Main()\nOpen \"anything\" For Output As #1\nEnd Sub").Success, "File statements refused");
try { _ = new Nanshe.ObjectModel.Range(new Application(fake), 0, "A1:XFD1048576"); throw new Exception("Unbounded range accepted"); }
catch (ArgumentOutOfRangeException) { passed++; }
var ranged = new Nanshe.ObjectModel.Range(new Application(fake), 0, "$B$2:C3");
Check(ranged.Count == 4 && ranged.Offset(1, 1).Address == "C3:D4", "Rectangular range offsets");
try { ranged.Value = new object[1, 1]; throw new Exception("Wrong-sized array accepted"); }
catch (ArgumentException) { passed++; }
var t = Transpiler.Transpile(new("Module1", VbaModuleKind.Standard, "Sub Main()\n  Dim a As Integer, b As Long, c As Currency\nEnd Sub"));
Check(t.VbSource.Contains("As Short") && t.VbSource.Contains("As Integer") && t.VbSource.Contains("Decimal"), "VBA integer and currency widths retained");
Check(t.LineMap.Count == t.VbSource.Split('\n').Length - 1, "Source mapping retained");
Console.WriteLine($"{passed} checks passed");

sealed class FakeEngine : IEngine
{
    public List<(string Command, JsonObject Params)> Calls { get; } = [];
    public JsonNode? Execute(string command, JsonObject parameters)
    {
        Calls.Add((command, parameters));
        return command switch {
            "document.inspect" => JsonNode.Parse("""{"title":"Book1","activeSheet":0,"sheets":[{"name":"Sheet1","usedRange":"A1:C3"}],"selection":"A1","activeCell":"A1"}"""),
            "sheet.read" => JsonNode.Parse("""{"values":[[42]]}"""),
            _ => null,
        };
    }
}
