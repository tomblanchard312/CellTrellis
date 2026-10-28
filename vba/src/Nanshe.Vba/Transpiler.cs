using System.Text;
using System.Text.RegularExpressions;

namespace Nanshe.Vba;

/// <summary>Standard module, class module, a document's code (ThisWorkbook) or a UserForm.</summary>
public enum VbaModuleKind { Standard, Class, Document, Form }

/// <summary>One VBA module as exported from the VBA editor (.bas / .cls) or read from vbaProject.bin.</summary>
public sealed record VbaModule(string Name, VbaModuleKind Kind, string Source)
{
    public static VbaModule FromFile(string path)
    {
        var text = File.ReadAllText(path);
        var kind = Path.GetExtension(path).Equals(".cls", StringComparison.OrdinalIgnoreCase) ? VbaModuleKind.Class : VbaModuleKind.Standard;
        var name = Regex.Match(text, @"^Attribute\s+VB_Name\s*=\s*""([^""]+)""", RegexOptions.Multiline) is { Success: true } m
            ? m.Groups[1].Value
            : Path.GetFileNameWithoutExtension(path);
        return new VbaModule(name, kind, text);
    }
}

/// <summary>A module translated to VB.NET. LineMap[i] is the VBA line (1-based) behind VB line i+1; 0 for generated lines.</summary>
public sealed record TranspiledModule(string Name, string VbSource, IReadOnlyList<int> LineMap, IReadOnlyList<string> Warnings);

public sealed class TranspileException(string module, int line, string message) : Exception($"{module} line {line}: {message}")
{
    public string Module { get; } = module;
    public int Line { get; } = line;
}

/// <summary>
/// Translates VBA source to VB.NET that Roslyn compiles with Option Strict Off (late binding),
/// Option Explicit Off and On Error intact. The changes are the known VBA → VB.NET differences:
/// integer widths, ByRef by default, Set/Let/Wend/Type, parenthesis-free calls, untyped Dim lists,
/// array lower bounds, and library calls that need the host (MsgBox, Format, IsMissing…).
/// </summary>
public static partial class Transpiler
{
    static readonly HashSet<string> Keywords = new(StringComparer.OrdinalIgnoreCase)
    {
        "If", "Else", "ElseIf", "End", "For", "Next", "Do", "Loop", "While", "Wend", "Until", "Select", "Case", "With",
        "Dim", "Private", "Public", "Global", "Static", "Const", "ReDim", "Erase", "Exit", "GoTo", "GoSub", "Return",
        "On", "Resume", "Sub", "Function", "Property", "Type", "Enum", "Option", "Call", "Set", "Let", "Open", "Close",
        "Print", "Input", "Line", "Get", "Put", "Write", "Name",
        "Stop", "Implements", "Event", "RaiseEvent", "Declare", "Friend", "Rem", "Error", "Lock",
        "Unlock", "Seek", "Width", "Load", "Unload", "LSet", "RSet", "Debug", "Is", "Not", "New",
    };

    static readonly Dictionary<string, string> TypeMap = new(StringComparer.OrdinalIgnoreCase)
    {
        ["Integer"] = "Short",
        ["Long"] = "Integer",
        ["LongLong"] = "Long",
        ["LongPtr"] = "Long",
        ["Currency"] = "Decimal",
        ["Variant"] = "Object",
    };

    sealed class Ctx(VbaModule module)
    {
        public VbaModule Module { get; } = module;
        public bool OptionExplicit;
        public bool CompareText;
        public bool InType;
        public bool HasClassInitialize;
        public int Line;
        public List<string> Warnings { get; } = [];
        public void Warn(string w) => Warnings.Add($"{Module.Name} line {Line}: {w}");
        public TranspileException Error(string message) => new(Module.Name, Line, message);
    }

    public static TranspiledModule Transpile(VbaModule module)
    {
        var ctx = new Ctx(module);
        var body = new List<(string Text, int Line)>();
        foreach (var (logical, line) in LogicalLines(module.Source))
        {
            ctx.Line = line;
            var output = TransformLine(logical, ctx);
            if (output is not null) body.Add((output, line));
        }

        var vb = new StringBuilder();
        var map = new List<int>();
        void Emit(string text, int line) { vb.Append(text).Append('\n'); map.Add(line); }

        Emit("Option Strict Off", 0);
        Emit(ctx.OptionExplicit ? "Option Explicit On" : "Option Explicit Off", 0);
        Emit(ctx.CompareText ? "Option Compare Text" : "Option Compare Binary", 0);
        Emit("Option Infer Off", 0);
        Emit(module.Kind == VbaModuleKind.Class ? $"Public Class {module.Name}" : $"Public Module {module.Name}", 0);
        if (ctx.HasClassInitialize) Emit("    Public Sub New() : Class_Initialize() : End Sub", 0);
        foreach (var (text, line) in body) Emit("    " + text, line);
        Emit(module.Kind == VbaModuleKind.Class ? "End Class" : "End Module", 0);
        return new TranspiledModule(module.Name, vb.ToString(), map, ctx.Warnings);
    }

    // ---- lines ----------------------------------------------------------------------------

    /// <summary>Logical lines (continuations joined), without export headers and Attribute lines.</summary>
    static IEnumerable<(string Text, int Line)> LogicalLines(string source)
    {
        var lines = source.Replace("\r\n", "\n").Replace('\r', '\n').Split('\n');
        int i = 0;
        // .cls / .frm exports start with VERSION … and a BEGIN … END block.
        if (i < lines.Length && lines[i].TrimStart().StartsWith("VERSION ", StringComparison.OrdinalIgnoreCase)) i++;
        if (i < lines.Length && lines[i].Trim().Equals("BEGIN", StringComparison.OrdinalIgnoreCase))
        {
            while (i < lines.Length && !lines[i].Trim().Equals("END", StringComparison.OrdinalIgnoreCase)) i++;
            i++;
        }
        for (; i < lines.Length; i++)
        {
            int start = i + 1;
            var sb = new StringBuilder(lines[i].TrimEnd());
            while (sb.Length >= 2 && sb[^1] == '_' && char.IsWhiteSpace(sb[^2]) && i + 1 < lines.Length)
            {
                sb.Length -= 1;
                sb.Append(lines[++i].Trim());
            }
            var text = sb.ToString();
            if (text.TrimStart().StartsWith("Attribute ", StringComparison.OrdinalIgnoreCase)) continue;
            yield return (text, start);
        }
    }

    static string? TransformLine(string line, Ctx ctx)
    {
        var (code, comment) = SplitComment(line);
        var indent = code[..(code.Length - code.TrimStart().Length)];
        var trimmed = code.Trim();
        if (trimmed.Length == 0) return line.Length == 0 ? "" : indent + comment.TrimStart();
        if (Regex.IsMatch(trimmed, @"^Rem(\s|$)", RegexOptions.IgnoreCase)) return indent + "'" + trimmed[3..] + comment;

        var statements = SplitStatements(trimmed);
        var outParts = new List<string>();
        foreach (var st in statements)
        {
            var t = TransformStatement(st.Trim(), ctx);
            if (t is not null) outParts.Add(t);
        }
        if (outParts.Count == 0) return comment.Length > 0 ? indent + comment : null;
        return indent + string.Join(" : ", outParts) + comment;
    }

    /// <summary>Code and the trailing ' comment (kept verbatim), split outside string literals.</summary>
    static (string Code, string Comment) SplitComment(string line)
    {
        bool inString = false;
        for (int i = 0; i < line.Length; i++)
        {
            if (line[i] == '"') inString = !inString;
            else if (line[i] == '\'' && !inString) return (line[..i], " " + line[i..]);
        }
        return (line, "");
    }

    /// <summary>Statements separated by ':' (not ':=', not inside strings; a single-line If keeps its tail).</summary>
    static List<string> SplitStatements(string code)
    {
        var parts = new List<string>();
        bool inString = false;
        int depth = 0, start = 0;
        for (int i = 0; i < code.Length; i++)
        {
            char c = code[i];
            if (c == '"') inString = !inString;
            if (inString) continue;
            if (c == '(') depth++;
            else if (c == ')') depth--;
            else if (c == ':' && depth == 0 && (i + 1 >= code.Length || code[i + 1] != '='))
            {
                var piece = code[start..i];
                if (IsSingleLineIf(piece)) continue; // the rest of the line belongs to the If
                if (parts.Count == 0 && LabelRx().IsMatch(piece)) { parts.Add(piece + ":"); start = i + 1; continue; }
                parts.Add(piece);
                start = i + 1;
            }
        }
        if (start < code.Length) parts.Add(code[start..]);
        return parts.Where(p => p.Trim().Length > 0).ToList();
    }

    [GeneratedRegex(@"^\s*[A-Za-z_]\w*\s*$")]
    private static partial Regex LabelRx();

    static bool IsSingleLineIf(string s) => SingleIfRx().IsMatch(s.Trim());

    [GeneratedRegex(@"^If\s+.+?\s+Then\s+\S", RegexOptions.IgnoreCase)]
    private static partial Regex SingleIfRx();

    // ---- statements --------------------------------------------------------------------------

    static string? TransformStatement(string s, Ctx ctx)
    {
        if (s.EndsWith(':') && LabelRx().IsMatch(s[..^1])) return s;
        var first = FirstWord(s);
        var lower = s.ToLowerInvariant();

        // Options move to the file header.
        if (first.Equals("Option", StringComparison.OrdinalIgnoreCase))
        {
            if (Regex.IsMatch(s, @"^Option\s+Explicit$", RegexOptions.IgnoreCase)) ctx.OptionExplicit = true;
            else if (Regex.IsMatch(s, @"^Option\s+Compare\s+Text$", RegexOptions.IgnoreCase)) ctx.CompareText = true;
            else if (Regex.IsMatch(s, @"^Option\s+Base\s+1$", RegexOptions.IgnoreCase)) ctx.Warn("Option Base 1: arrays keep index 0 too; code that relies on LBound = 1 may need a look.");
            return null;
        }
        if (Regex.IsMatch(s, @"^Def(Bool|Byte|Int|Lng|LngLng|LngPtr|Cur|Sng|Dbl|Dec|Date|Str|Obj|Var)\s", RegexOptions.IgnoreCase))
        {
            ctx.Warn($"{first} is ignored; undeclared variables are Variant.");
            return null;
        }
        if (Regex.IsMatch(s, @"^((Public|Private)\s+)?Declare\s", RegexOptions.IgnoreCase))
            throw ctx.Error("Declare calls native Windows code, which Nanshe macros can't run.");
        if (Regex.IsMatch(s, @"^(Open\s.+\sFor\s|Close(\s+#|$)|(Print|Write|Input|Get|Put|Seek|Lock|Unlock|Width)\s+#|Line\s+Input\s+#)", RegexOptions.IgnoreCase))
            throw ctx.Error("File I/O statements aren't available to macros.");
        if (first.Equals("GoSub", StringComparison.OrdinalIgnoreCase))
            throw ctx.Error("GoSub isn't supported; move that code into a Sub.");

        // Type … End Type → Structure.
        var typeHead = Regex.Match(s, @"^((Public|Private)\s+)?Type\s+(\w+)$", RegexOptions.IgnoreCase);
        if (typeHead.Success) { ctx.InType = true; return $"{(typeHead.Groups[2].Success ? typeHead.Groups[2].Value : "Public")} Structure {typeHead.Groups[3].Value}"; }
        if (Regex.IsMatch(s, @"^End\s+Type$", RegexOptions.IgnoreCase)) { ctx.InType = false; return "End Structure"; }
        if (ctx.InType) return "Public " + TransformDeclItem(s, ctx, isConst: false, isReDim: false);

        if (lower == "wend") return "End While";

        if (Regex.IsMatch(s, @"^(Set|Let)\s+", RegexOptions.IgnoreCase))
            return TransformStatement(s[(first.Length)..].TrimStart(), ctx);

        if (Regex.IsMatch(s, @"^((Public|Private|Friend|Global|Static)\s+)*(Sub|Function|Property)\s", RegexOptions.IgnoreCase))
            return TransformProcedure(s, ctx);

        if (Regex.IsMatch(s, @"^(Dim|Private|Public|Global|Static|Const)\s", RegexOptions.IgnoreCase) && !Regex.IsMatch(s, @"^(Public|Private)\s+Enum\s", RegexOptions.IgnoreCase))
            return TransformDeclaration(s, ctx);
        if (Regex.IsMatch(s, @"^ReDim\s", RegexOptions.IgnoreCase))
            return TransformReDim(s, ctx);

        if (Regex.IsMatch(s, @"^Debug\.Print\b", RegexOptions.IgnoreCase))
            return "VbaHost.DebugPrint(" + string.Join(", ", SplitTopLevel(s[11..].Trim().TrimEnd(';'), ';', ',').Select(a => Rename(a.Trim())).Where(a => a.Length > 0)) + ")";
        if (Regex.IsMatch(s, @"^Debug\.Assert\b", RegexOptions.IgnoreCase)) { ctx.Warn("Debug.Assert is ignored."); return null; }

        var singleIf = Regex.Match(s, @"^If\s+(.+?)\s+Then\s+(\S.*)$", RegexOptions.IgnoreCase);
        if (singleIf.Success)
        {
            var (thenPart, elsePart) = SplitElse(singleIf.Groups[2].Value);
            string Inner(string part) => string.Join(" : ", SplitStatements(part).Select(p => TransformStatement(p.Trim(), ctx)).Where(p => p is not null));
            var result = $"If {Rename(singleIf.Groups[1].Value)} Then {Inner(thenPart)}";
            return elsePart is null ? result : $"{result} Else {Inner(elsePart)}";
        }

        if (Regex.IsMatch(s, @"^(Private|Public|Friend)?\s*Event\s", RegexOptions.IgnoreCase))
            return Rename(s);

        return Rename(FixCall(s));
    }

    static (string Then, string? Else) SplitElse(string tail)
    {
        bool inString = false;
        for (int i = 0; i < tail.Length - 4; i++)
        {
            if (tail[i] == '"') inString = !inString;
            if (!inString && i > 0 && char.IsWhiteSpace(tail[i - 1]) && string.Compare(tail, i, "Else", 0, 4, StringComparison.OrdinalIgnoreCase) == 0
                && (i + 4 == tail.Length || char.IsWhiteSpace(tail[i + 4])))
                return (tail[..i].Trim(), tail[(i + 4)..].Trim());
        }
        return (tail, null);
    }

    /// <summary>VBA calls a Sub without parentheses (<c>Foo a, b</c>); VB.NET needs <c>Foo(a, b)</c>.</summary>
    internal static string FixCall(string s)
    {
        int i = 0;
        if (i < s.Length && s[i] == '.') i++;
        int firstStart = i;
        string? firstIdent = null;
        while (i < s.Length)
        {
            int idStart = i;
            if (!(char.IsLetter(s[i]) || s[i] == '_')) return s;
            while (i < s.Length && (char.IsLetterOrDigit(s[i]) || s[i] == '_')) i++;
            if (i < s.Length && "$%&!#@".Contains(s[i])) i++;
            firstIdent ??= s[idStart..i];
            if (i < s.Length && s[i] == '(')
            {
                int close = MatchParen(s, i);
                if (close < 0) return s;
                i = close + 1;
            }
            if (i < s.Length && s[i] == '.') { i++; continue; }
            break;
        }
        if (firstIdent is null || (firstStart == 0 && Keywords.Contains(firstIdent))) return s;
        var callee = s[..i];
        var rest = s[i..].Trim();
        if (rest.Length == 0) return s;                         // Foo / obj.Method
        if (rest[0] == '=' ) return s;                          // assignment
        if (rest.StartsWith(":=")) return s;
        if (rest[0] == '(' && MatchParen(rest, 0) == rest.Length - 1) return s; // Foo (x)
        return $"{callee}({rest})";
    }

    static int MatchParen(string s, int open)
    {
        int depth = 0;
        bool inString = false;
        for (int i = open; i < s.Length; i++)
        {
            if (s[i] == '"') inString = !inString;
            if (inString) continue;
            if (s[i] == '(') depth++;
            else if (s[i] == ')' && --depth == 0) return i;
        }
        return -1;
    }

    static string FirstWord(string s) => Regex.Match(s, @"^[A-Za-z_][\w]*").Value;

    // ---- procedures --------------------------------------------------------------------------

    static string TransformProcedure(string s, Ctx ctx)
    {
        var m = Regex.Match(s, @"^((?:(?:Public|Private|Friend|Global|Static)\s+)*)(Sub|Function|Property\s+(?:Get|Let|Set))\s+(\w+)(.*)$", RegexOptions.IgnoreCase);
        if (!m.Success) throw ctx.Error($"Can't read the procedure header: {s}");
        var kind = m.Groups[2].Value;
        if (kind.StartsWith("Property", StringComparison.OrdinalIgnoreCase))
            throw ctx.Error("Property Get/Let/Set procedures aren't supported yet.");
        var mods = m.Groups[1].Value;
        if (Regex.IsMatch(mods, @"\bStatic\b", RegexOptions.IgnoreCase))
        {
            ctx.Warn("Static on a procedure is ignored; declare the variables that must persist with Static.");
            mods = Regex.Replace(mods, @"\bStatic\s+", "", RegexOptions.IgnoreCase);
        }
        mods = Regex.Replace(mods, @"\bGlobal\b", "Public", RegexOptions.IgnoreCase);
        var name = m.Groups[3].Value;
        if (ctx.Module.Kind == VbaModuleKind.Class && name.Equals("Class_Initialize", StringComparison.OrdinalIgnoreCase)) ctx.HasClassInitialize = true;
        if (name.Equals("Class_Terminate", StringComparison.OrdinalIgnoreCase)) ctx.Warn("Class_Terminate doesn't run when the last reference goes away.");

        var rest = m.Groups[4].Value.Trim();
        string parameters = "", returns = "";
        if (rest.StartsWith('('))
        {
            int close = MatchParen(rest, 0);
            if (close < 0) throw ctx.Error("Unbalanced parentheses in the parameter list.");
            parameters = TransformParameters(rest[1..close], ctx);
            rest = rest[(close + 1)..].Trim();
        }
        var asMatch = Regex.Match(rest, @"^As\s+(.+)$", RegexOptions.IgnoreCase);
        if (asMatch.Success) returns = " As " + MapType(asMatch.Groups[1].Value.Trim());
        else if (kind.Equals("Function", StringComparison.OrdinalIgnoreCase)) returns = " As Object";
        if (mods.Length == 0) mods = "Public ";
        return $"{mods}{kind} {name}({parameters}){returns}";
    }

    static string TransformParameters(string list, Ctx ctx)
    {
        if (list.Trim().Length == 0) return "";
        var output = new List<string>();
        foreach (var raw in SplitTopLevel(list, ','))
        {
            var p = raw.Trim();
            var m = Regex.Match(p, @"^(Optional\s+)?(ByVal\s+|ByRef\s+)?(ParamArray\s+)?(\w+[$%&!#@]?)(\(\s*\))?(?:\s+As\s+([\w.]+(?:\s*\*\s*\w+)?))?(?:\s*=\s*(.+))?$", RegexOptions.IgnoreCase);
            if (!m.Success) throw ctx.Error($"Can't read parameter '{p}'.");
            bool optional = m.Groups[1].Success, paramArray = m.Groups[3].Success;
            string mode = paramArray ? "ByVal " : m.Groups[2].Success ? m.Groups[2].Value : "ByRef ";
            string type = m.Groups[6].Success ? MapType(m.Groups[6].Value) : "Object";
            string def = m.Groups[7].Success ? " = " + Rename(m.Groups[7].Value.Trim()) : optional ? (type.Equals("String", StringComparison.OrdinalIgnoreCase) ? " = \"\"" : " = Nothing") : "";
            output.Add($"{(optional ? "Optional " : "")}{mode}{(paramArray ? "ParamArray " : "")}{m.Groups[4].Value}{m.Groups[5].Value} As {type}{def}");
        }
        return string.Join(", ", output);
    }

    // ---- declarations ------------------------------------------------------------------------

    static string TransformDeclaration(string s, Ctx ctx)
    {
        var m = Regex.Match(s, @"^((?:(?:Public|Private|Global|Dim|Static)\s+)*)(Const\s+)?(WithEvents\s+)?(.*)$", RegexOptions.IgnoreCase);
        var mods = Regex.Replace(m.Groups[1].Value, @"\bGlobal\b", "Public", RegexOptions.IgnoreCase);
        bool isConst = m.Groups[2].Success;
        var items = SplitTopLevel(m.Groups[4].Value, ',').Select(i => TransformDeclItem(i.Trim(), ctx, isConst, isReDim: false));
        return $"{mods}{(isConst ? "Const " : "")}{m.Groups[3].Value}{string.Join(", ", items)}";
    }

    static string TransformReDim(string s, Ctx ctx)
    {
        var m = Regex.Match(s, @"^ReDim\s+(Preserve\s+)?(.*)$", RegexOptions.IgnoreCase);
        var items = SplitTopLevel(m.Groups[2].Value, ',').Select(i => TransformDeclItem(i.Trim(), ctx, isConst: false, isReDim: true));
        return $"ReDim {m.Groups[1].Value}{string.Join(", ", items)}";
    }

    static string TransformDeclItem(string item, Ctx ctx, bool isConst, bool isReDim)
    {
        var m = Regex.Match(item, @"^(\w+[$%&!#@]?)\s*(\((.*)\))?(?:\s+As\s+(New\s+)?([\w.]+)(\s*\*\s*\w+)?)?(?:\s*=\s*(.+))?$", RegexOptions.IgnoreCase);
        if (!m.Success) throw ctx.Error($"Can't read declaration '{item}'.");
        var name = m.Groups[1].Value;
        var bounds = m.Groups[2].Success ? "(" + TransformBounds(m.Groups[3].Value) + ")" : "";
        if (m.Groups[6].Success) ctx.Warn($"Fixed-length String {name} becomes a variable-length String.");
        string type = m.Groups[5].Success ? MapType(m.Groups[5].Value) : "";
        string value = m.Groups[7].Success ? " = " + Rename(m.Groups[7].Value.Trim()) : "";
        if (isReDim) return name + bounds; // VB.NET's ReDim takes no As clause
        if (type.Length == 0) return isConst ? $"{name}{value}" : $"{name}{bounds} As Object{value}";
        return $"{name}{bounds} As {(m.Groups[4].Success ? "New " : "")}{type}{value}";
    }

    /// <summary>VB.NET arrays start at 0: <c>(1 To 5, 0 To 2)</c> becomes <c>(5, 2)</c>.</summary>
    // shortcut: lower bounds retain index zero, preserve array bounds before claiming full VBA array parity.
    static string TransformBounds(string bounds) =>
        bounds.Trim().Length == 0 ? "" : string.Join(", ", SplitTopLevel(bounds, ',').Select(b =>
        {
            var to = Regex.Match(b, @"^(.+?)\s+To\s+(.+)$", RegexOptions.IgnoreCase);
            return Rename((to.Success ? to.Groups[2].Value : b).Trim());
        }));

    static string MapType(string t)
    {
        t = Regex.Replace(t.Trim(), @"\s*\*\s*\w+$", "");
        return TypeMap.TryGetValue(t, out var mapped) ? mapped : t;
    }

    static List<string> SplitTopLevel(string s, params char[] separators)
    {
        var parts = new List<string>();
        int depth = 0, start = 0;
        bool inString = false;
        for (int i = 0; i < s.Length; i++)
        {
            char c = s[i];
            if (c == '"') inString = !inString;
            if (inString) continue;
            if (c == '(') depth++;
            else if (c == ')') depth--;
            else if (depth == 0 && separators.Contains(c)) { parts.Add(s[start..i]); start = i + 1; }
        }
        parts.Add(s[start..]);
        return parts;
    }

    // ---- names -------------------------------------------------------------------------------

    static readonly (Regex Rx, string To)[] Renames =
    [
        (new(@"(?<![.\w])(Left|Right|Mid|Trim|LTrim|RTrim|UCase|LCase|Chr|ChrW|Space|Str|Hex|Oct)\$", RegexOptions.IgnoreCase), "$1"),
        (new(@"(?<![.\w])String\$?\s*\(", RegexOptions.IgnoreCase), "StrDup("),
        (new(@"(?<![.\w])Format\$?\s*\(", RegexOptions.IgnoreCase), "VbaHost.Format("),
        (new(@"(?<![.\w])MsgBox(?!\w)", RegexOptions.IgnoreCase), "VbaHost.MsgBox"),
        (new(@"(?<![.\w])InputBox(?!\w)", RegexOptions.IgnoreCase), "VbaHost.InputBox"),
        (new(@"(?<![.\w])IsMissing(?!\w)", RegexOptions.IgnoreCase), "VbaHost.IsMissing"),
        (new(@"(?<![.\w])IsEmpty(?!\w)", RegexOptions.IgnoreCase), "VbaHost.IsEmpty"),
        (new(@"(?<![.\w])IsNull(?!\w)", RegexOptions.IgnoreCase), "VbaHost.IsNull"),
        (new(@"(?<![.\w])Null(?!\w)", RegexOptions.IgnoreCase), "VbaHost.Null"),
        (new(@"(?<![.\w])Empty(?!\w)", RegexOptions.IgnoreCase), "Nothing"),
        (new(@"(?<![.\w])CInt\s*\(", RegexOptions.IgnoreCase), "CShort("),
        (new(@"(?<![.\w])CLng\s*\(", RegexOptions.IgnoreCase), "CInt("),
        (new(@"(?<![.\w])CLngLng\s*\(", RegexOptions.IgnoreCase), "CLng("),
        (new(@"(?<![.\w])CLngPtr\s*\(", RegexOptions.IgnoreCase), "CLng("),
        (new(@"(?<![.\w])CVar\s*\(", RegexOptions.IgnoreCase), "CObj("),
        (new(@"(?<![.\w])CCur\s*\(", RegexOptions.IgnoreCase), "CDec("),
        (new(@"(?<![.\w])ThisWorkbook(?!\w)", RegexOptions.IgnoreCase), "ActiveWorkbook"),
        (new(@"(?<![.\w])(?<!\bAs\s+)(?<!\bNew\s+)Date\$?(?![\w(])", RegexOptions.IgnoreCase), "Today"),
        (new(@"(?<![.\w])(?<!\bAs\s+)Time\$?(?![\w(])", RegexOptions.IgnoreCase), "TimeOfDay"),
    ];

    /// <summary>Applies the library renames to code outside string literals.</summary>
    internal static string Rename(string code)
    {
        var sb = new StringBuilder();
        int i = 0;
        while (i < code.Length)
        {
            int q = code.IndexOf('"', i);
            var segment = q < 0 ? code[i..] : code[i..q];
            foreach (var (rx, to) in Renames) segment = rx.Replace(segment, to);
            sb.Append(segment);
            if (q < 0) break;
            int end = q + 1;
            while (end < code.Length)
            {
                if (code[end] == '"' && end + 1 < code.Length && code[end + 1] == '"') { end += 2; continue; }
                if (code[end] == '"') break;
                end++;
            }
            sb.Append(code, q, Math.Min(code.Length, end + 1) - q);
            i = end + 1;
        }
        return sb.ToString();
    }
}
