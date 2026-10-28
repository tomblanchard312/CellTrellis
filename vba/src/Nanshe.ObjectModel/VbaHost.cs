using System.Globalization;
using System.Text;
namespace Nanshe.ObjectModel;

/// <summary>
/// VBA library functions that need a host (dialogs, Debug.Print) or behave differently from
/// VB.NET's versions (Format's date codes, IsMissing). The transpiler routes calls here.
/// </summary>
public static class VbaHost
{
    [ThreadStatic] static Action<string>? _output;
    [ThreadStatic] static Func<string, string, string, string?>? _input;

    /// <summary>Where MsgBox and Debug.Print write. Defaults to the console.</summary>
    public static Action<string> Output { get => _output ?? Console.WriteLine; set => _output = value; }

    /// <summary>Answers InputBox(prompt, title, default); null keeps the default.</summary>
    public static Func<string, string, string, string?>? Input { get => _input; set => _input = value; }

    public static object MsgBox(object Prompt, object? Buttons = null, object? Title = null)
    {
        var title = Title is null ? "" : $"{Title}: ";
        Output($"[MsgBox] {title}{Prompt}");
        return 1; // vbOK
    }

    public static string InputBox(object Prompt, object? Title = null, object? Default = null, object? XPos = null, object? YPos = null)
    {
        var d = Convert.ToString(Default, CultureInfo.InvariantCulture) ?? "";
        return Input?.Invoke(Convert.ToString(Prompt) ?? "", Convert.ToString(Title) ?? "", d) ?? d;
    }

    public static void DebugPrint(params object?[] items) => Output(string.Concat(items.Select(i => Convert.ToString(i, CultureInfo.CurrentCulture))));

    public static bool IsMissing(object? value) => value is null;
    public static bool IsEmpty(object? value) => value is null;
    public static bool IsNull(object? value) => value is DBNull;
    public static object Null => DBNull.Value;

    /// <summary>VBA's Format: named formats and VBA date codes (mm is month, nn minutes) mapped to .NET.</summary>
    public static string Format(object? Expression, object? Style = null, object? FirstDayOfWeek = null, object? FirstWeekOfYear = null)
    {
        var style = Convert.ToString(Style, CultureInfo.InvariantCulture) ?? "";
        if (Expression is null) return "";
        if (Expression is DateTime dt)
        {
            return style.ToLowerInvariant() switch
            {
                "" or "general date" => dt.ToString("G", CultureInfo.CurrentCulture),
                "short date" => dt.ToString("d", CultureInfo.CurrentCulture),
                "long date" => dt.ToString("D", CultureInfo.CurrentCulture),
                "short time" => dt.ToString("HH:mm", CultureInfo.CurrentCulture),
                "long time" => dt.ToString("T", CultureInfo.CurrentCulture),
                _ => dt.ToString(DateCodes(style), CultureInfo.CurrentCulture),
            };
        }
        if (Expression is IConvertible && double.TryParse(Convert.ToString(Expression, CultureInfo.InvariantCulture), NumberStyles.Any, CultureInfo.InvariantCulture, out var n) && Expression is not string)
        {
            return style.ToLowerInvariant() switch
            {
                "" or "general number" => n.ToString(CultureInfo.CurrentCulture),
                "currency" => n.ToString("C", CultureInfo.CurrentCulture),
                "fixed" => n.ToString("0.00", CultureInfo.CurrentCulture),
                "standard" => n.ToString("#,##0.00", CultureInfo.CurrentCulture),
                "percent" => (n * 100).ToString("0.00", CultureInfo.CurrentCulture) + "%",
                "yes/no" => n != 0 ? "Yes" : "No",
                "true/false" => n != 0 ? "True" : "False",
                "on/off" => n != 0 ? "On" : "Off",
                _ => n.ToString(style, CultureInfo.CurrentCulture),
            };
        }
        return Microsoft.VisualBasic.Strings.Format(Expression, style);
    }

    /// <summary>VBA date codes to .NET: m/mm/mmm/mmmm are months unless they follow h (minutes); n/nn are minutes; AM/PM is tt.</summary>
    static string DateCodes(string vba)
    {
        var sb = new StringBuilder();
        bool afterHour = false;
        for (int i = 0; i < vba.Length; i++)
        {
            char c = vba[i];
            int run = 1;
            while (i + run < vba.Length && char.ToLowerInvariant(vba[i + run]) == char.ToLowerInvariant(c)) run++;
            string token = vba.Substring(i, run);
            switch (char.ToLowerInvariant(c))
            {
                case 'm':
                    bool minutes = afterHour && run <= 2 || NextIsSeconds(vba, i + run);
                    sb.Append(minutes ? new string('m', run) : new string('M', run));
                    afterHour = false;
                    break;
                case 'n': sb.Append(new string('m', Math.Min(run, 2))); break;
                case 'h': sb.Append(new string('H', Math.Min(run, 2))); afterHour = true; break;
                case 'd': sb.Append(new string('d', Math.Min(run, 4))); afterHour = false; break;
                case 'y': sb.Append(run <= 2 ? "yy" : "yyyy"); afterHour = false; break;
                case 's': sb.Append(new string('s', Math.Min(run, 2))); break;
                case 'a' when vba.Length - i >= 5 && vba.Substring(i, 5).Equals("AM/PM", StringComparison.OrdinalIgnoreCase):
                    sb.Append("tt"); run = 5; break;
                case '/' or ':' or '-' or ' ' or '.' or ',': sb.Append(token); break;
                default: sb.Append('\'').Append(token.Replace("'", "")).Append('\''); break;
            }
            i += run - 1;
        }
        // AM/PM present: use the 12-hour clock.
        var s = sb.ToString();
        return s.Contains("tt") ? s.Replace("HH", "hh").Replace("H", "h") : s;
    }

    static bool NextIsSeconds(string vba, int from)
    {
        for (int i = from; i < vba.Length; i++)
        {
            char c = char.ToLowerInvariant(vba[i]);
            if (c == 's') return true;
            if (char.IsLetter(c)) return false;
        }
        return false;
    }
}
