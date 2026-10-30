# Scripting runtime packaging

VBA is bundled with its own .NET runtime and compiler. The installed GUI checks its adjacent
`runtimes/vba` first, then the app-specific FHS directory under `lib`. A development SDK is
not required on the user's machine. Run `python3 packaging/stage-runtimes.py --help` for the
stager used by every native packaging script. It includes runtime/compiler licences and
third-party notices, writes a target/component manifest, and compiles a synthetic macro on native
builds with external SDK lookup disabled.

Native Linux staging example:

```
python3 packaging/stage-runtimes.py --rid linux-x64 --out target/runtime-stage/runtimes
```

For a local developer install, copy the complete `runtimes` tree beside the versioned GUI
binary. Do not copy only the executable. Run the stager's `--verify-only` check against the
installed runtime directory. The source Flatpak manifest consumes this pre-staged directory;
tar/Flatpak/AppImage repackaging must preserve the application's `lib` tree.

Cross-builds are not native validation. FreeBSD and universal macOS builds require a tested,
self-contained prebuilt VBA directory with matching `runtime-info.json`, supplied through
`NANSHE_VBA_RUNTIME_DIR`. The stager fails rather than publishing a partial package. macOS
JIT entitlements apply to the helper, not the GUI.

CellTrellis also needs Python 3 and R with jsonlite. Debian/RPM packages declare these system
dependencies. A local install uses `--scripts system --check-system` to verify them. Windows,
macOS, FreeBSD and portable Linux packages use `--scripts bundle` and require
`NANSHE_SCRIPT_RUNTIME_DIR` containing tested, relocatable `python/` and `R/` distributions.
The canonical launchers are `python/python[.exe]` and `R/bin/Rscript[.exe]`; R must include
jsonlite. Preserve their licences, notices and the corresponding-source/offer obligations
of the redistributed versions. No runtimes are downloaded on application startup. Native
staging checks bundled jsonlite without borrowing the host's R library. Missing inputs stop
the package build. Foreign-platform bundle presence checks do not replace native smoke tests.
