# CellTrellis VBA

Nisaba's VBA-to-VB.NET transpiler and Roslyn compiler, adapted to an Excel-style spreadsheet
object model. MIT OR Apache-2.0; see the repository NOTICE for upstream and Nisaba credits.

Build `dotnet build vba/NansheVba.sln -m:1` from the repository root. Test with
`dotnet run --project vba/tests/Nanshe.Vba.Tests --no-build`.

Set `NANSHE_VBA_RUNNER` to the built `CellTrellis.Vba` executable (`.exe` on Windows) and open
Developer → Code Editor in CellTrellis. The helper uses private stdin/stdout to preview commands in a
copied workbook; only the user's Apply action changes the original.

For standalone translation: `dotnet run --project vba/src/Nanshe.Vba -- transpile module.bas`.

See [Developer setup, API, trust model and limitations](../docs/nanshe-developer.md).
