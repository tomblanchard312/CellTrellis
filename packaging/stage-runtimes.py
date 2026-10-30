#!/usr/bin/env python3
"""Stage complete native scripting runtimes; incomplete bundles fail the package build."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import platform
import tempfile
import sys

ROOT = Path(__file__).resolve().parents[1]
CELL = (ROOT / 'vba/src/Nanshe.Vba').is_dir()
PROJECT = ROOT / ('vba/src/Nanshe.Vba' if CELL else 'vba/src/WordCraft.Vba')
NAME = 'Nanshe.Vba' if CELL else 'wcvba'
PREFIX = 'NANSHE' if CELL else 'NISABA'

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

def verify_vba(folder, rid):
    exe = folder / (NAME + ('.exe' if rid.startswith('win-') else ''))
    require(exe.is_file(), f'Missing VBA launcher: {exe}')
    metadata = folder / 'runtime-info.json'
    require(metadata.is_file() and json.loads(metadata.read_text()).get('rid') == rid, 'VBA runtime target does not match the package target')
    files = list(folder.rglob('*'))
    require(any(p.name == 'Microsoft.CodeAnalysis.VisualBasic.dll' for p in files), 'VBA bundle lacks the compiler')
    require(any('hostfxr' in p.name for p in files), 'VBA bundle must include its .NET runtime, not just the apphost')
    for component in ['coreclr', 'hostpolicy']:
        require(any(p.name.lower() in [component + '.dll', 'lib' + component + '.so', 'lib' + component + '.dylib'] for p in files),
                'VBA bundle lacks native runtime component: ' + component)
    require(any(p.name == 'System.Private.CoreLib.dll' for p in files), 'VBA bundle lacks the core framework library')
    require((folder / (NAME + '.runtimeconfig.json')).is_file(), 'VBA bundle lacks its runtime configuration')
    require(any('license' in p.name.lower() for p in files), 'VBA bundle lacks redistribution licences')
    require(any('notice' in p.name.lower() for p in files), 'VBA bundle lacks third-party notices')

def notices(folder):
    dest = folder / 'notices'
    dest.mkdir(exist_ok=True)
    dotnet = Path(shutil.which('dotnet') or '').resolve().parent
    sources = [(dotnet, 'dotnet-sdk')]
    cache = Path(os.environ.get('NUGET_PACKAGES', Path.home() / '.nuget/packages'))
    for deps in folder.glob('*.deps.json'):
        for library in json.loads(deps.read_text()).get('libraries', {}):
            package = library.removeprefix('runtimepack.').lower()
            if '/' in package:
                sources.append((cache / package, package.replace('/', '-')))
    for source, label in sources:
        if not source.is_dir():
            continue
        for path in source.iterdir():
            if path.is_file() and (path.name.lower().startswith(('license', 'copyright')) or 'notice' in path.name.lower()):
                shutil.copy2(path, dest / (label + '-' + path.name))

def system_scripts():
    python = shutil.which('python3') or shutil.which('python')
    r = shutil.which('Rscript')
    require(python and r, 'Install Python 3, R and jsonlite before installing CellTrellis')
    subprocess.run([r, '--vanilla', '-e', 'stopifnot(requireNamespace("jsonlite", quietly=TRUE))'], check=True, timeout=30)

def native_rid():
    system = {'Linux':'linux','Windows':'win','Darwin':'osx','FreeBSD':'freebsd'}.get(platform.system(), '')
    arch = {'x86_64':'x64','AMD64':'x64','amd64':'x64','aarch64':'arm64','arm64':'arm64'}.get(platform.machine(), platform.machine())
    return system + '-' + arch

def probe_vba(folder):
    exe = folder / (NAME + ('.exe' if os.name == 'nt' else ''))
    env = dict(os.environ, DOTNET_ROOT=str(folder / 'no-external-sdk'), DOTNET_MULTILEVEL_LOOKUP='0')
    source = 'Sub Main()\nDebug.Print "runtime-ready"\nEnd Sub\n'
    if CELL:
        result = subprocess.run([str(exe.resolve()), 'bridge'], input=json.dumps({'source':source,'macro':'Main'})+'\n',
                                text=True, capture_output=True, timeout=30, env=env, check=True)
        frames = [json.loads(line) for line in result.stdout.splitlines()]
        require(any(f.get('type') == 'done' for f in frames), 'VBA compilation smoke test did not finish')
    else:
        with tempfile.TemporaryDirectory(prefix='vba-package-check-') as temp:
            path = Path(temp) / 'check.bas'; path.write_text(source)
            subprocess.run([str(exe.resolve()), 'check', str(path)], env=env, check=True, capture_output=True, timeout=30)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rid', required=True)
    parser.add_argument('--out', type=Path, required=True, help='application runtime directory')
    parser.add_argument('--scripts', choices=['system', 'bundle', 'none'], default='none')
    parser.add_argument('--check-system', action='store_true')
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    folder = args.out / 'vba'
    if not args.verify_only:
        supplied = os.environ.get(PREFIX + '_VBA_RUNTIME_DIR')
        if supplied:
            source = Path(supplied).resolve()
            require(source != folder.resolve(), 'VBA source and destination must differ')
            verify_vba(source, args.rid)
            if folder.exists(): shutil.rmtree(folder)
            shutil.copytree(source, folder)
        else:
            require(args.rid.split('-')[0] in ['linux', 'win', 'osx'] and not args.rid.endswith('universal'),
                    f'{args.rid} requires a native, self-contained {PREFIX}_VBA_RUNTIME_DIR bundle')
            if folder.exists(): shutil.rmtree(folder)
            folder.mkdir(parents=True)
            subprocess.run(['dotnet', 'publish', str(PROJECT), '-c', 'Release', '-r', args.rid,
                            '--self-contained', 'true', '-o', str(folder.resolve())], check=True)
            notices(folder)
            (folder / "runtime-info.json").write_text(json.dumps({"rid": args.rid, "helper": NAME}) + "\n")
    verify_vba(folder, args.rid)
    if CELL and args.scripts == 'bundle':
        supplied = os.environ.get('NANSHE_SCRIPT_RUNTIME_DIR')
        if not args.verify_only:
            require(supplied, 'Portable packages require NANSHE_SCRIPT_RUNTIME_DIR containing Python, R, jsonlite and their redistribution/source notices')
            source = Path(supplied)
            for name in ['python', 'R']:
                target = args.out / name
                require((source / name).is_dir(), f'Missing bundled {name}')
                if target.exists(): shutil.rmtree(target)
                shutil.copytree(source / name, target)
        suffix = '.exe' if args.rid.startswith('win-') else ''
        for file in ['python/python' + suffix, 'R/bin/Rscript' + suffix]:
            require((args.out / file).is_file(), f'Missing scripting runtime: {file}')
        require(any((args.out / 'R').rglob('jsonlite/DESCRIPTION')), 'Bundled R lacks jsonlite')
        for name in ['python', 'R']:
            require(any('licen' in p.name.lower() or 'copying' in p.name.lower() for p in (args.out / name).rglob('*')),
                    f'{name} bundle lacks redistribution licences')
    native = args.rid == native_rid() or (args.rid == 'osx-universal' and platform.system() == 'Darwin')
    if native:
        probe_vba(folder)
        if CELL and args.scripts == 'bundle':
            rhome = (args.out / 'R').resolve()
            env = dict(os.environ, R_HOME=str(rhome), R_LIBS_USER=str(rhome / 'library'), R_LIBS_SITE=str(rhome / 'site-library'))
            subprocess.run([str((rhome / ('bin/Rscript.exe' if os.name == 'nt' else 'bin/Rscript')).resolve()), '--vanilla', '-e',
                            'stopifnot(requireNamespace("jsonlite",quietly=TRUE));stopifnot(startsWith(normalizePath(find.package("jsonlite")),normalizePath(Sys.getenv("R_HOME"))))'],
                           env=env, check=True, timeout=30)
    if args.check_system:
        system_scripts()
    if not args.verify_only:
        files = sorted(str(p.relative_to(args.out)) for p in args.out.rglob('*') if p.is_file() and p.name != 'manifest.json')
        (args.out / 'manifest.json').write_text(json.dumps({'rid': args.rid, 'vba': NAME, 'scripts': args.scripts, 'files': files}, indent=2) + '\n')
    print(f'Runtime bundle verified: {args.out}')

if __name__ == '__main__':
    try: main()
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f'Runtime staging failed: {error}', file=sys.stderr)
        sys.exit(1)
