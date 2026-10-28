using System.Collections;
using System.Globalization;
using System.Runtime.CompilerServices;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace Nanshe.ObjectModel;

public interface IEngine
{
    JsonNode? Execute(string command, JsonObject parameters);
}

public static class ExcelGlobals
{
    [ThreadStatic] static Application? _current;
    public static Application Current
    {
        get => _current ?? throw new InvalidOperationException("No workbook attached.");
        set => _current = value;
    }
}

public static class XlConstants
{
    public const int xlUp = -4162, xlDown = -4121, xlToLeft = -4159, xlToRight = -4161;
}

/// <summary>One preview workbook; the runner has no file or desktop-control capability.</summary>
public sealed class Application(IEngine engine)
{
    internal JsonNode? Call(string command, JsonObject? parameters = null) => engine.Execute(command, parameters ?? new());
    internal JsonNode Inspect => Call("document.inspect") ?? throw new InvalidOperationException("Workbook unavailable.");
    public string Name => "Nanshe";
    public Workbook ActiveWorkbook => new(this);
    public Workbooks Workbooks => new(this);
    public Worksheet ActiveSheet => new(this, (int)Inspect["activeSheet"]!);
    public Worksheets Worksheets => new(this);
    public Worksheets Sheets => Worksheets;
    public Range Selection => ActiveSheet.Range((string)Inspect["selection"]!);
    public Range ActiveCell => ActiveSheet.Range((string)Inspect["activeCell"]!);
    public Range Range(string Cell1, object? Cell2 = null) => ActiveSheet.Range(Cell1, Cell2);
    public Range Cells(int Row, int Column) => ActiveSheet.Cells(Row, Column);
}

public sealed class Workbook(Application app)
{
    public string Name => (string?)app.Inspect["title"] ?? "Workbook";
    public Worksheets Worksheets => new(app);
    public Worksheets Sheets => Worksheets;
    public Worksheet ActiveSheet => app.ActiveSheet;
    public void Activate() { }
}

public sealed class Workbooks(Application app) : IEnumerable
{
    public int Count => 1;
    public Workbook this[object index] => index is string name
        ? name.Equals(app.ActiveWorkbook.Name, StringComparison.OrdinalIgnoreCase) ? app.ActiveWorkbook : throw new ArgumentException("Only the active workbook is exposed.")
        : Convert.ToInt32(index, CultureInfo.InvariantCulture) == 1 ? app.ActiveWorkbook : throw new ArgumentOutOfRangeException(nameof(index));
    public IEnumerator GetEnumerator() { yield return app.ActiveWorkbook; }
}

public sealed class Worksheets(Application app) : IEnumerable
{
    public int Count => app.Inspect["sheets"]!.AsArray().Count;
    public Worksheet this[object index]
    {
        get
        {
            var sheets = app.Inspect["sheets"]!.AsArray();
            int i = index is string name
                ? sheets.Select((s, n) => (s, n)).Where(x => string.Equals((string?)x.s?["name"], name, StringComparison.OrdinalIgnoreCase)).Select(x => x.n).DefaultIfEmpty(-1).First()
                : checked(Convert.ToInt32(index, CultureInfo.InvariantCulture) - 1);
            if (i < 0 || i >= sheets.Count) throw new ArgumentOutOfRangeException(nameof(index), "Worksheet not found.");
            return new(app, i);
        }
    }
    public IEnumerator GetEnumerator() { for (int i = 1; i <= Count; i++) yield return this[i]; }
}

public sealed class Worksheet(Application app, int index)
{
    public int Index => index + 1;
    public string Name => (string)app.Inspect["sheets"]![index]!["name"]!;
    public void Activate() => app.Call("sheet.activate", new() { ["sheet"] = index });
    public Range Range(string Cell1, object? Cell2 = null) => new(app, index, Cell2 is null ? Cell1 : Cell1 + ":" + Convert.ToString(Cell2, CultureInfo.InvariantCulture));
    public Range Cells(int Row, int Column) => new(app, index, Nanshe.ObjectModel.Range.AddressOf(Row, Column));
    public Range UsedRange => Range((string?)app.Inspect["sheets"]![index]?["usedRange"] ?? "A1");
}

public sealed class Range : IEnumerable
{
    readonly Application app;
    readonly int sheet, row, col, height, width;
    public Range(Application app, int sheet, string address)
    {
        this.app = app; this.sheet = sheet;
        var m = Regex.Match(address, @"^\$?([A-Za-z]{1,3})\$?([1-9][0-9]{0,6})(?::\$?([A-Za-z]{1,3})\$?([1-9][0-9]{0,6}))?$", RegexOptions.CultureInvariant);
        if (!m.Success) throw new ArgumentException("Use a rectangular A1 range; select the worksheet separately.");
        static int Column(string text) => text.ToUpperInvariant().Aggregate(0, (n, c) => checked(n * 26 + c - 'A' + 1));
        row = int.Parse(m.Groups[2].Value, CultureInfo.InvariantCulture); col = Column(m.Groups[1].Value);
        int endRow = m.Groups[4].Success ? int.Parse(m.Groups[4].Value, CultureInfo.InvariantCulture) : row;
        int endCol = m.Groups[3].Success ? Column(m.Groups[3].Value) : col;
        height = checked(endRow - row + 1); width = checked(endCol - col + 1);
        if (height < 1 || width < 1 || endRow > 1_048_576 || endCol > 16_384 || (long)height * width > 10_000)
            throw new ArgumentOutOfRangeException(nameof(address), "Ranges must fit the sheet and contain at most 10,000 cells.");
    }
    internal static string AddressOf(int row, int col)
    {
        if (row < 1 || row > 1_048_576 || col < 1 || col > 16_384) throw new ArgumentOutOfRangeException(nameof(row));
        string letters = "";
        while (col > 0) { col--; letters = (char)('A' + col % 26) + letters; col /= 26; }
        return letters + row.ToString(CultureInfo.InvariantCulture);
    }
    public string Address => AddressOf(row, col) + (Count == 1 ? "" : ":" + AddressOf(row + height - 1, col + width - 1));
    public int Row => row;
    public int Column => col;
    public int Count => height * width;
    public Range this[int Row, int Column = 1] => Cells(Row, Column);
    public Range Cells(int Row, int Column = 1)
    {
        if (Row < 1 || Row > height || Column < 1 || Column > width) throw new ArgumentOutOfRangeException(nameof(Row));
        return new(app, sheet, AddressOf(row + Row - 1, col + Column - 1));
    }
    public Range Offset(int RowOffset = 0, int ColumnOffset = 0) => new(app, sheet,
        AddressOf(checked(row + RowOffset), checked(col + ColumnOffset)) + ":" + AddressOf(checked(row + RowOffset + height - 1), checked(col + ColumnOffset + width - 1)));
    public Range Resize(int RowSize, int ColumnSize) => RowSize > 0 && ColumnSize > 0
        ? new(app, sheet, AddressOf(row, col) + ":" + AddressOf(checked(row + RowSize - 1), checked(col + ColumnSize - 1)))
        : throw new ArgumentOutOfRangeException(nameof(RowSize));
    JsonObject Params() => new() { ["sheet"] = sheet, ["range"] = Address };
    internal void Set(string command, string key, object? value)
    {
        var p = Params(); p[key] = System.Text.Json.JsonSerializer.SerializeToNode(value); app.Call(command, p);
    }
    public object? Value { get => Read(false); set => Write(value, false); }
    public object? Value2 { get => Value; set => Value = value; }
    public object? Formula { get => Read(true); set => Write(value, true); }
    object? Read(bool formulas)
    {
        var p = Params(); p["formulas"] = formulas;
        var values = app.Call("sheet.read", p)?["values"]?.AsArray() ?? new();
        static object? Scalar(JsonNode? v) => v is JsonValue j ? j.TryGetValue<bool>(out var b) ? b : j.TryGetValue<double>(out var n) ? n : j.ToString() : v?.ToJsonString();
        if (Count == 1) return Scalar(values[0]?[0]);
        var result = Array.CreateInstance(typeof(object), new[] { height, width }, new[] { 1, 1 });
        for (int r = 0; r < height; r++) for (int c = 0; c < width; c++) result.SetValue(Scalar(values[r]?[c]), r + 1, c + 1);
        return result;
    }
    void Write(object? value, bool formulas)
    {
        if (value is Array a && (a.Rank != 2 || a.GetLength(0) != height || a.GetLength(1) != width))
            throw new ArgumentException("Array dimensions must match the range.");
        var rows = new JsonArray();
        for (int r = 0; r < height; r++)
        {
            var cells = new JsonArray();
            for (int c = 0; c < width; c++)
            {
                object? v = value is Array data ? data.GetValue(r + data.GetLowerBound(0), c + data.GetLowerBound(1)) : value;
                // Excel's Value stores text literally; Formula explicitly opts into formula parsing.
                if (!formulas && v is string text) v = "'" + text;
                cells.Add(System.Text.Json.JsonSerializer.SerializeToNode(v));
            }
            rows.Add(cells);
        }
        var p = Params(); p["values"] = rows; p["parse"] = !formulas;
        app.Call("range.setValues", p);
    }
    public void Select() => app.Call("selection.set", Params());
    public void ClearContents() => app.Call("edit.clearContents", Params());
    public Font Font => new(this);
    public string NumberFormat { set => Set("home.numberFormat", "code", value); }
    public IEnumerator GetEnumerator() { for (int r = 1; r <= height; r++) for (int c = 1; c <= width; c++) yield return Cells(r, c); }
}

public sealed class Font(Range range)
{
    public bool Bold { set => range.Set("home.bold", "on", value); }
    public bool Italic { set => range.Set("home.italic", "on", value); }
    public double Size { set => range.Set("home.fontSize", "size", value); }
    public string Name { set => range.Set("home.fontName", "name", value); }
}
