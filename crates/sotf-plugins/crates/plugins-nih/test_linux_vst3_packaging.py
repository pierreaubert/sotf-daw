"""Focused tests for native Linux VST3 bundle architecture packaging."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


JUSTFILE = Path(__file__).with_name("Justfile")


@unittest.skipUnless(shutil.which("just"), "just is required to exercise the packaging recipe")
class LinuxVst3PackagingTests(unittest.TestCase):
    def run_packager(self, host_arch: str, target_triplet: str | None = None,
                     existing_output: bool = False, host_os: str = "Linux") -> tuple[subprocess.CompletedProcess[str], Path]:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        source = root / "dist/nih-linux/libsotf_eq.so"
        source.parent.mkdir(parents=True)
        source.write_bytes(b"native-library-fixture")
        old_output = root / "dist/vst3-linux/old.vst3/retained.txt"
        if existing_output:
            old_output.parent.mkdir(parents=True)
            old_output.write_text("keep if target validation fails")

        # The standalone plugin Justfile expects a workspace-level `cargo`
        # variable; packaging itself does not invoke Cargo.
        justfile = root / "Justfile"
        justfile.write_text('cargo := "cargo"\n' + JUSTFILE.read_text())
        fake_bin = root / "fake-bin"
        fake_bin.mkdir()
        uname = fake_bin / "uname"
        uname.write_text(
            "#!/usr/bin/env bash\n"
            "if [[ $1 == -s ]]; then echo " + host_os + "; else echo " + host_arch + "; fi\n"
        )
        uname.chmod(0o755)

        env = os.environ.copy()
        env["PATH"] = f"{fake_bin}{os.pathsep}{env.get('PATH', '')}"
        env.pop("SOTF_LINUX_TARGET_TRIPLET", None)
        if target_triplet is not None:
            env["SOTF_LINUX_TARGET_TRIPLET"] = target_triplet
        result = subprocess.run(
            [shutil.which("just"), "--justfile", str(justfile), "--working-directory", str(root),
             "_package-vst3-linux"],
            cwd=root, env=env, text=True, capture_output=True, check=False,
        )
        return result, root

    def assert_bundle_arch(self, root: Path, triplet: str) -> None:
        bundles = list((root / "dist/vst3-linux").glob("*.vst3"))
        self.assertEqual(len(bundles), 1)
        binary = bundles[0] / "Contents" / triplet / f"{bundles[0].stem}.so"
        self.assertEqual(binary.read_bytes(), b"native-library-fixture")
        other_triplet = "aarch64-linux" if triplet == "x86_64-linux" else "x86_64-linux"
        self.assertFalse((bundles[0] / "Contents" / other_triplet).exists())

    def test_x86_64_host_keeps_existing_default_layout(self) -> None:
        result, root = self.run_packager("x86_64")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assert_bundle_arch(root, "x86_64-linux")

    def test_aarch64_host_uses_explicit_native_triplet(self) -> None:
        result, root = self.run_packager("aarch64", "aarch64-linux")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assert_bundle_arch(root, "aarch64-linux")

    def test_aarch64_host_defaults_to_native_triplet(self) -> None:
        result, root = self.run_packager("aarch64")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assert_bundle_arch(root, "aarch64-linux")

    def test_foreign_or_unsupported_target_fails_before_removing_old_output(self) -> None:
        for host_arch, target, expected in (
            ("x86_64", "aarch64-linux", "does not match native host"),
            ("aarch64", "armv7-linux", "Unsupported Linux VST3 target triplet"),
        ):
            with self.subTest(host_arch=host_arch, target=target):
                result, root = self.run_packager(host_arch, target, existing_output=True)
                self.assertEqual(result.returncode, 2, result.stderr + result.stdout)
                self.assertIn(expected, result.stderr + result.stdout)
                self.assertEqual(
                    (root / "dist/nih-linux/libsotf_eq.so").read_bytes(),
                    b"native-library-fixture",
                )
                self.assertEqual(
                    (root / "dist/vst3-linux/old.vst3/retained.txt").read_text(),
                    "keep if target validation fails",
                )

    def test_non_linux_host_fails_before_packaging(self) -> None:
        result, root = self.run_packager("x86_64", host_os="Darwin")
        self.assertEqual(result.returncode, 2, result.stderr + result.stdout)
        self.assertIn("requires a Linux host", result.stderr + result.stdout)
        self.assertFalse((root / "dist/vst3-linux").exists())


if __name__ == "__main__":
    unittest.main()
