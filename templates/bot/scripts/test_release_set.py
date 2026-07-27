from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import release_set


class ReleaseSetTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(__file__).resolve().parents[1]
        self.release = release_set.load_release(
            release_set.discover_active(self.root / "releases")
        )

    def test_active_manifest_and_product_lock_are_coherent(self) -> None:
        release_set.validate_active_set(self.release, self.root)

    def test_duplicate_core_revision_is_rejected(self) -> None:
        expected = self.release.by_id["core"].revision
        with tempfile.TemporaryDirectory() as directory:
            lock = Path(directory) / "Cargo.lock"
            lock.write_text(
                "\n".join(
                    [
                        f'source = "git+https://github.com/sena-nana/MutsukiCore.git?rev={expected}#{expected}"',
                        'source = "git+https://github.com/sena-nana/MutsukiCore.git?rev=0000000000000000000000000000000000000000#0000000000000000000000000000000000000000"',
                    ]
                ),
                encoding="utf-8",
            )
            with self.assertRaises(release_set.ReleaseSetError):
                release_set.validate_lock_core(self.release, lock)

    def test_sync_updates_managed_git_pins(self) -> None:
        service = self.release.by_id["service_host"]
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            for repository in self.release.repositories:
                checkout = release_set.repository_path(workspace, repository)
                checkout.mkdir()
                (checkout / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            manifest = release_set.repository_path(workspace, service) / "Cargo.toml"
            manifest.write_text(
                f'[dependencies]\nx = {{ git = "{service.url}", rev = "0000000000000000000000000000000000000000" }}\n',
                encoding="utf-8",
            )
            changed = release_set.sync_workspace(self.release, workspace, update_locks=False)
            self.assertIn(manifest.parent, changed)
            self.assertIn(service.revision, manifest.read_text(encoding="utf-8"))

    def test_deployment_revision_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            deployments = Path(directory)
            (deployments / "stale.toml").write_text(
                '[external_service]\nrevision = "0000000000000000000000000000000000000000"\n',
                encoding="utf-8",
            )
            with self.assertRaises(release_set.ReleaseSetError):
                release_set.validate_deployment_pins(self.release, deployments)


if __name__ == "__main__":
    unittest.main()
