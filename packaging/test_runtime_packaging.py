import importlib.util
import json
from pathlib import Path
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('stage_runtimes', ROOT / 'packaging/stage-runtimes.py')
stage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stage)

class RuntimePackagingTests(unittest.TestCase):
    def test_incomplete_or_wrong_target_vba_bundles_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            with self.assertRaises(RuntimeError): stage.verify_vba(folder, 'linux-x64')
            for name in [stage.NAME, 'Microsoft.CodeAnalysis.VisualBasic.dll', 'libhostfxr.so', 'libcoreclr.so', 'libhostpolicy.so', 'System.Private.CoreLib.dll', stage.NAME + '.runtimeconfig.json', 'LICENSE.txt', 'THIRD-PARTY-NOTICES.txt']:
                (folder / name).write_text('fixture')
            (folder / 'runtime-info.json').write_text(json.dumps({'rid':'linux-x64'}))
            stage.verify_vba(folder, 'linux-x64')
            with self.assertRaises(RuntimeError): stage.verify_vba(folder, 'freebsd-x64')
            (folder / 'Microsoft.CodeAnalysis.VisualBasic.dll').unlink()
            with self.assertRaises(RuntimeError): stage.verify_vba(folder, 'linux-x64')

    def test_freebsd_native_rid_uses_the_dotnet_architecture_name(self):
        with patch.object(stage.platform, 'system', return_value='FreeBSD'), patch.object(stage.platform, 'machine', return_value='amd64'):
            self.assertEqual(stage.native_rid(), 'freebsd-x64')

    def test_native_package_builders_stage_the_runtime(self):
        for file in ['linux/package.sh', 'windows/package.ps1', 'freebsd/package.sh', 'macos/package.sh']:
            self.assertIn('stage-runtimes.py', (ROOT / 'packaging' / file).read_text(), file)
        for manifest in (ROOT / 'packaging/linux/flatpak').glob('*.yml'):
            self.assertIn('runtimes', manifest.read_text(), str(manifest))
        installer = next((ROOT / 'packaging/windows').glob('*.wxs'))
        self.assertIn('runtimes\\', installer.read_text())

if __name__ == '__main__':
    unittest.main()
