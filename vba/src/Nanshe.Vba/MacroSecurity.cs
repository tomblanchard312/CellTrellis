using System.Reflection;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.VisualBasic;
using Microsoft.CodeAnalysis.VisualBasic.Syntax;
using Nanshe.ObjectModel;

namespace Nanshe.Vba;

/// <summary>
/// Static allow-list over the compiled macro. Every symbol the code names, and the static type of
/// every expression, must come from the macro itself, Nanshe's object model or a small slice
/// of the base library; late-bound member names must be members of those types.
///
/// This is defence in depth, not the boundary: .NET has no in-process sandbox, so the runner is
/// meant to execute inside an OS sandbox whose only capability is the Nanshe connection.
/// </summary>
public static class MacroSecurity
{
    static readonly HashSet<string> SystemTypes = new(StringComparer.Ordinal)
    {
        "Object", "String", "Boolean", "Byte", "SByte", "Char", "Int16", "Int32", "Int64", "UInt16", "UInt32", "UInt64",
        "Single", "Double", "Decimal", "DateTime", "TimeSpan", "Math", "Convert", "DBNull", "Nullable", "Void", "ValueType",
        "Array", "Random", "StringComparison", "MidpointRounding", "DayOfWeek",
    };

    static readonly HashSet<string> VisualBasicTypes = new(StringComparer.Ordinal)
    {
        "Strings", "Conversion", "DateAndTime", "Information", "Financial", "VBMath", "Constants", "ControlChars",
        "Collection", "ErrObject", "TriState", "CompareMethod", "VbStrConv", "MsgBoxResult", "MsgBoxStyle",
        "FirstDayOfWeek", "FirstWeekOfYear", "DateInterval", "DueDate", "VariantType", "Interaction",
    };

    /// <summary>Interaction members that only compute values; the rest (Shell, CreateObject, Environ, CallByName, registry…) are denied.</summary>
    static readonly HashSet<string> SafeInteraction = new(StringComparer.Ordinal) { "IIf", "Choose", "Switch", "Partition", "Beep" };

    static readonly HashSet<string> DeniedMembers = new(StringComparer.OrdinalIgnoreCase)
    {
        "GetType", "MemberwiseClone", "Finalize", "ReferenceEquals",
    };

    static readonly Lazy<HashSet<string>> LateBoundNames = new(() =>
    {
        var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var types = typeof(Application).Assembly.GetExportedTypes()
            .Where(t => t.Namespace == "Nanshe.ObjectModel")
            .Append(typeof(string))
            .Append(typeof(Microsoft.VisualBasic.Collection));
        foreach (var t in types)
            foreach (var m in t.GetMembers(BindingFlags.Public | BindingFlags.Instance | BindingFlags.Static))
                names.Add(m.Name);
        names.Add("Item");
        names.ExceptWith(DeniedMembers);
        return names;
    });

    public static IReadOnlyList<Diagnostic> Check(Compilation compilation)
    {
        var problems = new List<Diagnostic>();
        foreach (var tree in compilation.SyntaxTrees)
        {
            var model = compilation.GetSemanticModel(tree);
            foreach (var node in tree.GetRoot().DescendantNodes())
            {
                if (node is not (ExpressionSyntax or TypeSyntax)) continue;
                var info = model.GetSymbolInfo(node);
                var symbol = info.Symbol ?? info.CandidateSymbols.FirstOrDefault();
                if (symbol is not null && !Allowed(symbol, compilation.Assembly))
                    problems.Add(Problem(node, $"'{symbol.ToDisplayString()}' isn't available to macros."));
                if (node is ExpressionSyntax && model.GetTypeInfo(node).Type is ITypeSymbol t && !TypeAllowed(t, compilation.Assembly))
                    problems.Add(Problem(node, $"Values of type '{t.ToDisplayString()}' aren't available to macros."));
                // obj.Member with obj As Object binds at run time: only members of the object model are allowed.
                if (node is MemberAccessExpressionSyntax ma && symbol is null && info.CandidateSymbols.IsEmpty
                    && !LateBoundNames.Value.Contains(ma.Name.Identifier.ValueText))
                    problems.Add(Problem(node, $"Late-bound member '{ma.Name.Identifier.ValueText}' isn't part of Nanshe's object model."));
            }
        }
        return problems.DistinctBy(d => (d.Location.SourceSpan, d.GetMessage())).ToList();
    }

    static Diagnostic Problem(SyntaxNode node, string message) =>
        Diagnostic.Create(new DiagnosticDescriptor("WCVBA001", "Macro security", "{0}", "Security", DiagnosticSeverity.Error, true), node.GetLocation(), message);

    static bool Allowed(ISymbol symbol, IAssemblySymbol macro)
    {
        switch (symbol)
        {
            case ILocalSymbol or IParameterSymbol or ILabelSymbol or IRangeVariableSymbol or INamespaceSymbol:
                return true;
            case IArrayTypeSymbol a:
                return TypeAllowed(a.ElementType, macro);
        }
        if (SymbolEqualityComparer.Default.Equals(symbol.ContainingAssembly, macro)) return true;
        if (DeniedMembers.Contains(symbol.Name) && symbol is IMethodSymbol) return false;
        var type = symbol as INamedTypeSymbol ?? symbol.ContainingType;
        if (type is null) return false;
        if (!TypeAllowed(type, macro)) return false;
        if (type.ContainingNamespace?.ToDisplayString() == "Microsoft.VisualBasic" && type.Name == "Interaction")
            return symbol is INamedTypeSymbol || SafeInteraction.Contains(symbol.Name);
        return true;
    }

    static bool TypeAllowed(ITypeSymbol type, IAssemblySymbol macro)
    {
        if (type is IArrayTypeSymbol a) return TypeAllowed(a.ElementType, macro);
        if (type is ITypeParameterSymbol or IErrorTypeSymbol) return true;
        if (SymbolEqualityComparer.Default.Equals(type.ContainingAssembly, macro)) return true;
        var ns = type.ContainingNamespace?.ToDisplayString() ?? "";
        var name = type.OriginalDefinition.Name;
        if (type is INamedTypeSymbol { ContainingType: { } outer }) return TypeAllowed(outer, macro);
        return ns switch
        {
            "Nanshe.ObjectModel" => true,
            "System" => SystemTypes.Contains(name),
            "System.Collections" or "System.Collections.Generic" => true,
            "System.Text" => name == "StringBuilder",
            "Microsoft.VisualBasic" => VisualBasicTypes.Contains(name),
            _ => false,
        };
    }
}
