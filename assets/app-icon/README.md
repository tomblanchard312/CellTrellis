# CellTrellis application icon

Original blue-and-gold folded spreadsheet, created for this project on 2026-10-29.
The navy tile, folded blue page and cyan highlights match the Nabu/Nisaba family;
gold headers and a highlighted cell distinguish the spreadsheet application.
The source is editable vector artwork in `nanshe.svg`, licensed MIT OR Apache-2.0.

Run `packaging/icons.sh` with resvg and Cargo. macOS uses iconutil; other hosts use
Python 3 to write a PNG-backed ICNS container. No runtime image tools are required.

Outputs: 1024px preview, macOS runtime PNG and ICNS, Windows nine-size ICO, and
Linux/FreeBSD hicolor PNG sizes 16, 24, 32, 48, 64, 128, 256 and 512 plus scalable SVG.
Existing application, web favicon and installer paths consume these files.
macOS renditions retain the platform's transparent margins.
