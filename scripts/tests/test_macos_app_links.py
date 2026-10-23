import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "macos_app_links", Path(__file__).resolve().parents[1] / "verify-macos-app-links.py"
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MacOSAppLinksTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / "source.app"
        self.copy = Path(self.temp.name) / "copy.app"
        for name in ("App", "FlutterMacOS"):
            framework = self.source / "Contents" / "Frameworks" / f"{name}.framework"
            version = framework / "Versions" / "A"
            version.mkdir(parents=True)
            (version / name).write_bytes(b"framework-binary")
            (framework / "Versions" / "Current").symlink_to("A", target_is_directory=True)
            (framework / name).symlink_to(Path("Versions") / "Current" / name)

    def test_preserved_links(self):
        shutil.copytree(self.source, self.copy, symlinks=True)
        self.assertEqual(module.verify_links(self.source, self.copy), 4)

    @unittest.skipUnless(sys.platform == "darwin", "ditto requires macOS")
    def test_ditto_and_pkg_staging_preserve_links(self):
        subprocess.run(["ditto", str(self.source), str(self.copy)], check=True)
        self.assertEqual(module.verify_links(self.source, self.copy), 4)
        staging = Path(self.temp.name) / "pkg-root" / "Applications" / "copy.app"
        subprocess.run(["ditto", str(self.copy), str(staging)], check=True)
        self.assertEqual(module.verify_links(self.source, staging), 4)

    def test_expanded_links_rejected(self):
        shutil.copytree(self.source, self.copy, symlinks=False)
        with self.assertRaisesRegex(ValueError, "expanded"):
            module.verify_links(self.source, self.copy)

    def test_flattened_source_rejected(self):
        shutil.copytree(self.source, self.copy, symlinks=False)
        with self.assertRaisesRegex(ValueError, "Expected framework symlink"):
            module.verify_links(self.copy, self.copy)

    def test_changed_target_rejected(self):
        shutil.copytree(self.source, self.copy, symlinks=True)
        link = self.copy / "Contents/Frameworks/App.framework/App"
        link.unlink()
        link.symlink_to(Path("Versions") / "A" / "App")
        with self.assertRaisesRegex(ValueError, "changed"):
            module.verify_links(self.source, self.copy)

    def test_broken_link_rejected(self):
        shutil.copytree(self.source, self.copy, symlinks=True)
        (self.copy / "Contents/Frameworks/App.framework/Versions/A/App").unlink()
        with self.assertRaises(FileNotFoundError):
            module.verify_links(self.source, self.copy)


if __name__ == "__main__":
    unittest.main()
