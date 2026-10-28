# Developer and Scribe in Nanshe

Last updated: 2026-10-28.

## Compared implementations

The port starts at GridCraft `70327cd3d9b7de9af1bcaa33488c0736a0d346d8` and takes the reusable
language/compiler and local-service client from Nisaba
`a4298e6fc1ecfdedac64cfe763943737c608b33a`.

| Area | Nisaba | GridCraft at the fork | Nanshe |
|---|---|---|---|
| Developer interface | Optional VBA CLI; no integrated editor | Recorded command scripts; preserves VBA projects | In-app source editor, language selector, runtime output, review and apply |
| VBA | VBA → VB.NET → Roslyn; Word object model | Preserved but not executed | Reused transpiler/security checks, Excel-style object model, isolated preview process |
| Python / R | No reusable execution integration | Python editor highlighting only | Local interpreters, shared command bridge, range read/write helpers |
| Scribe | Local SharedAI v2 client, Word tracked changes | No assistant | Local Enki v2 client, formula proposals and VBA/Python/R drafts |
| Edits | Word command protocol | Spreadsheet commands and undo groups | Existing spreadsheet commands, atomic grouped replay with document/revision checks |

Enki retains `sharedai-daemon`, `SHAREDAI_*`, and `SharedAI` paths. Nanshe uses those exact
identifiers; it does not embed a second inference engine. No dependency on the private Enki
repository is needed to compile Nanshe. The protocol client comes from public Nisaba source.

## Language runtimes

### VBA

Install a .NET SDK 8 or later and build:

```sh
dotnet build vba/NansheVba.sln -m:1
# Use the directory for your SDK's target framework (net8.0, net9.0, net10.0...).
export NANSHE_VBA_RUNNER="$PWD/vba/src/Nanshe.Vba/bin/Debug/net10.0/Nanshe.Vba"
cargo run -p gridcraft
```

Windows: set `NANSHE_VBA_RUNNER` to `Nanshe.Vba.exe`. Keep the helper executable beside its
DLLs, runtime configuration and Roslyn assemblies. It is an optional component; Cargo does not
build it. It uses the VB compiler shipped in the SDK, with no new NuGet package dependencies.

Supported spreadsheet surface: Application, ActiveWorkbook/ThisWorkbook, Workbooks (the active
workbook only), ActiveSheet, Worksheets/Sheets by name or 1-based index, Range, Cells,
Selection, ActiveCell, Value/Value2, Formula, Select, ClearContents, Offset, Resize, UsedRange,
Font setters and NumberFormat setter. Multiple-cell reads return 1-based two-dimensional arrays;
array writes must match the destination dimensions. Value writes preserve text literally, while
Formula explicitly parses formulas. Sheet activation and reads see earlier changes through
GridCraft's actual calculation engine in the preview session.

```vb
Sub Main()
    Worksheets("Sheet1").Range("A1:A3").Value = 10
    Range("B1").Formula = "=SUM(A1:A3)"
    Range("B1").Font.Bold = True
    Debug.Print Range("B1").Value
End Sub
```

Nisaba's VBA language coverage and limitations remain: ByRef defaults, integer widths, Optional,
Set/Let, calls without parentheses and source-mapped diagnostics are retained. VBA lower-bound
array declarations still include index zero; Property Get/Let/Set, forms, COM, native declarations
and file statements are unsupported. The editor runs a standard source module and a selected
parameterless Sub. Existing binary VBA projects in XLSM remain preserved, not automatically
imported or executed. There is no debugger or Excel COM automation.

### Python

Python 3 is looked up as `python3` (`python` on Windows). Set `NANSHE_PYTHON` to the executable
inside a virtual environment if you need installed packages such as pandas or NumPy. The runner
uses Python's standard-library JSON support and isolated startup (`-I`); user-site packages,
PYTHONPATH and startup customization are not loaded. Packages installed in the selected virtual
environment can be imported normally.

```python
rows = nanshe.read("A1:A3", sheet="Sheet1")
total = sum(row[0] or 0 for row in rows)
nanshe.write("B1", [[total]])
nanshe.write("C1", [["=SUM(A1:A3)"]], formulas=True)
print(total)
```

`read` returns rows of scalar values. `write` takes rows of values (e.g. a dataframe's
`df.values.tolist()`). Strings are literal unless `formulas=True`. `nanshe.execute` also exposes
the bounded workbook command allow-list, never file/desktop/nested-automation commands.

### R

Install R and the `jsonlite` package in its library, then use `Rscript` or set `NANSHE_RSCRIPT` to
its full path. Nanshe does not install packages. Startup is `Rscript --vanilla`.

```r
rows <- sheet_read("A1:A3", sheet = "Sheet1")
total <- sum(unlist(rows), na.rm = TRUE)
sheet_write("B1", matrix(total, nrow = 1))
sheet_write("C1", matrix("=SUM(A1:A3)", nrow = 1), formulas = TRUE)
print(total)
```

`sheet_read` returns row lists. `sheet_write` accepts a matrix, dataframe, or list of row lists.
Columns retain their scalar types for dataframe writes. Strings are literal unless formulas are
requested. NA, NaN and infinite numbers are refused; use NULL cells in row lists for blanks.
Use the provided `print`/`cat` for output. Direct low-level writes to stdout break the wire
protocol and invalidate the preview.

## Preview, persistence and trust

Each runtime is a separate ordinary-user process with private stdin/stdout, not a listening
control port. Workbook requests execute against a copied engine session, with the original
selection, calculation state and protection settings. The original workbook stays untouched
until Apply; failures discard the preview. Apply replays the same allow-listed commands in a
single engine undo group, refusing a different document or revision. Invalid late commands roll
back earlier changes. The script itself is never run twice.

Limits: 128 KiB source, 1,000 engine calls, 10,000 cells per range, 256 KiB wire frames,
4 MiB total helper output, 64 KiB console output, and a 15-second runtime deadline. Stop, closing
the workbook, or dropping the job cancels the runner. Unix runners start in their own process group, which is killed on Stop/deadline; Windows uses
`taskkill /T /F`. The child is reaped even on protocol failure. Deliberately detached processes
and external side effects remain outside this lifecycle; this is not an OS resource sandbox.

Source for VBA, Python and R is stored separately using GridCraft's existing hidden-name storage.
It survives XLSX saves; edited drafts join the normal Save/Close flow. CSV exports cannot retain
script source. Code is never executed on open. Switching source language, editing code, or
accepting a Scribe code draft clears the run acknowledgement and invalidates the old preview.

Only run code you trust. Python/R imports have ordinary-user filesystem and network access;
external file/network side effects happen during preview and cannot be undone by the workbook.
Nisaba's VBA syntax/symbol allow-list is retained as defence in depth, not an OS sandbox. The
preview and replay allow-list protects the workbook command channel; it does not make arbitrary
local code safe. The GUI and helpers never request administrator rights.

## Scribe / Enki

Run a configured local Enki service and register a chat model using Enki's model workflow.
Refresh models connects to `SHAREDAI_SOCKET` or `$XDG_RUNTIME_DIR/sharedai/ipc.sock`, starting
`sharedai-daemon` from `SHAREDAI_DAEMON`, the executable's helper directory, or PATH if needed.
No models are downloaded by Nanshe.

Scribe sends only the user's request and at most 100 cells (32 KiB) from the captured selection to
that local endpoint. It streams replies off the UI thread and supports Stop. Nisaba's reasoning
tag filtering is reused; truncated, failed or cancelled replies never become actionable
suggestions. Formula proposals use canonical English formula syntax and target the cell captured
when the request began. A workbook revision change makes Apply fail. Code drafts only open in the
Developer editor and must be reviewed and run explicitly.

## Platforms and evidence boundaries

- Desktop scripting uses standard child-process pipes on Linux, macOS, Windows and BSD, with
  separately installed runtimes. Runtime paths containing spaces are passed as executable paths,
  never shell strings.
- The copied Enki client currently uses Unix sockets: Linux, macOS and BSD. Windows named-pipe
  support remains pending, and Scribe controls clearly report that limit.
- Web builds retain source editing/storage. Local subprocess execution and local Enki are disabled.
- Cross-platform workflow checks the .NET object model on Linux/macOS/Windows, and the complete
  Rust-to-runtime bridge on Linux. CI configuration is not evidence that a hosted run passed.
- A real Enki/model smoke test and native Windows/macOS/FreeBSD GUI execution remain required
  before claiming those end-to-end integrations validated.

## Testing

```sh
dotnet build vba/NansheVba.sln -m:1
dotnet run --project vba/tests/Nanshe.Vba.Tests --no-build
# Set this to exercise the real .NET subprocess in the Rust integration test.
export NANSHE_VBA_TEST_RUNNER="$NANSHE_VBA_RUNNER"
# Set NANSHE_RSCRIPT to exercise the R subprocess test (requires jsonlite).
cargo test -p gridcraft-ui-egui developer --lib
cargo test -p gridcraft-engine cmd::developer --lib
cargo test -p gridcraft-enki
cargo xtask ci
```

The Python bridge test runs with the configured interpreter or system Python. The real R/.NET
bridge tests are conditional on their explicit runtime environment variables; omitting those
variables is not evidence that the runtimes were exercised. The Enki test server is a local
protocol fixture, not a live model. See the PR for actual validation results.

## Control commands

The existing desktop control channel/MCP bridge can call `view.developer`, `view.scribe`,
`ui.developerPreview {language: "VBA"|"Python"|"R", source, macro?: "Main", trusted: true}`,
`ui.developerStatus`, `ui.developerStop`, `ui.scribeModels`, and
`ui.scribeAsk {model, prompt, mode: "Formula"|"Explain selection"|"Write VBA"|"Write Python"|"Write R"}`.
Preview and Scribe run asynchronously. Status returns `busy`, output status, `preview`, `reply`
and `suggestion`. Apply the returned preview/proposal with the engine's `developer.apply`
command. `developer.source` and `developer.saveSource` accept `language: "vba"|"python"|"r"`.
A caller must obtain the user's trust decision before setting `trusted: true`.

## Local validation · 2026-10-28

The full `cargo xtask ci` gate passed all ten steps with `umask 022`,
`CRAFT_FONTS_DIR` pinned to `storytold/craft-fonts`
`b076417b5880bd4154bd22f00695b1d28c6bea83`, and all runtime-test environment variables set.
The run reported 2,568 Rust tests/doctests passed and one ignored. The separate .NET test
program passed 19 checks. Developer and Scribe were rendered offscreen and visually inspected;
the strict accessibility check is also covered by a regression test.

The actual VBA helper, system Python 3, and R 4.3.3 with jsonlite 1.8.8 were exercised. R was
extracted into a temporary workspace from distribution packages and launched through a local
wrapper; it was not installed on the host. Enki was checked against a real Unix-socket protocol
fixture, not a live model. The inherited fresh-file permission test expects a restrictive mask;
it fails under this host's `umask 0002`. No file-saving implementation was changed in this port.
