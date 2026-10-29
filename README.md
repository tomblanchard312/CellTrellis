# Nanshe

A Rust spreadsheet with a Developer workspace for **VBA, Python and R**, and **Scribe**, a local
assistant powered by [Enki](https://github.com/tomblanchard312/Enki).

Nanshe is a fork of [GridCraft](https://github.com/storytold/gridcraft) by the ArtCraft team.
It keeps Nanshe's engine, command protocol, file formats and internal crate names so upstream
improvements remain easy to merge. The desktop uses a separate Nanshe settings profile, dark
surfaces and blue accents. About lists the upstream contributors and licenses.

## Run

```sh
cargo run -p nanshe -- --sample sales
```

Open **Developer → Code Editor**, choose VBA, Python or R, edit the script, acknowledge that you
trust it, and choose **Run preview**. Review the engine calls and output, then choose
**Apply script changes**. The changes apply together and can be undone together. A changed or
closed workbook invalidates its preview. Save source in the workbook with **Save in workbook**;
ordinary file saves and close prompts also include edited script drafts.

- VBA needs the optional .NET helper built from [`vba/`](vba/README.md).
- Python needs a local Python 3 interpreter (`NANSHE_PYTHON` selects a virtual environment).
- R needs a local R installation and the `jsonlite` package (`NANSHE_RSCRIPT` selects Rscript).
- Scribe needs local Enki and a registered chat model. Open **Review → Scribe** or
  **Developer → Scribe**, refresh models, and send a request. Responses can explain a selection,
  propose a formula, or draft any of the three script languages. Applying or running a suggestion
  always requires your action.

Python and R execute trusted local code with your account's permissions. The workbook preview is
not an operating-system sandbox. Read the [setup, examples and limits](docs/nanshe-developer.md).

## Checks

```sh
cargo xtask ci
dotnet build vba/NansheVba.sln -m:1
dotnet run --project vba/tests/Nanshe.Vba.Tests --no-build
```

The inherited Nanshe architecture and format documentation is in [`docs/`](docs/). Packaged
installer identifiers and artwork still follow upstream; this branch is a source-build port,
not a signed Nanshe release. The upstream self-update feed is disabled for this fork.

## License and credits

MIT OR Apache-2.0. Nanshe is copyright (c) 2026 ArtCraft Team and the GridCraft contributors;
Nisaba-derived and Nanshe changes are copyright (c) 2026 Tom Blanchard. See [NOTICE](NOTICE) and
[ATTRIBUTION.md](ATTRIBUTION.md). Nanshe is not sponsored or endorsed by the ArtCraft team.
