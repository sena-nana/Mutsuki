from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import export_template


class TemplateExportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.source = Path(__file__).resolve().parents[1]
        self.root_manifest = Path(__file__).resolve().parents[3] / "Cargo.toml"

    def test_manifest_uses_one_revision_for_every_mutsuki_dependency(self) -> None:
        revision = "a" * 40
        manifest = export_template.render_manifest(
            self.source,
            self.root_manifest,
            "https://github.com/sena-nana/Mutsuki.git",
            "rev",
            revision,
        )
        internal = [
            line
            for line in manifest.splitlines()
            if line.startswith("mutsuki-") and " git = " in line
        ]
        self.assertGreater(len(internal), 10)
        self.assertTrue(all(f'rev = "{revision}"' in line for line in internal))
        self.assertNotIn("MutsukiCore.git", manifest)
        release = export_template.render_release_manifest(
            "https://github.com/sena-nana/Mutsuki.git", "v0.1.0", revision
        )
        self.assertIn(f'revision = "{revision}"', release)
        self.assertEqual(release.count("[[repositories]]"), 1)

    def test_copy_filter_removes_obsolete_release_set_and_generated_state(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "template"
            destination.mkdir()
            shutil_ignore = export_template.ignored_paths(self.source)
            scripts = shutil_ignore(
                str(self.source / "scripts"),
                [
                    "export_template.py",
                    "release_set.py",
                    "test_export_template.py",
                    "test_release_set.py",
                ],
            )
            root = shutil_ignore(str(self.source), ["artifacts", "releases", "crates"])
            workflows = shutil_ignore(
                str(self.source / ".github" / "workflows"),
                ["platform-compat.yml", "release-set.yml"],
            )
            self.assertEqual(
                scripts,
                {
                    "export_template.py",
                    "release_set.py",
                    "test_export_template.py",
                    "test_release_set.py",
                },
            )
            self.assertEqual(workflows, {"release-set.yml"})
            self.assertEqual(root, {"artifacts", "releases"})

    def test_invalid_revision_is_rejected(self) -> None:
        with self.assertRaises(export_template.ExportError):
            export_template.validate_reference("rev", "main")

    def test_distribution_placeholders_are_materialized(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            deployment_dir = output / "deploy" / "distribution"
            deployment_dir.mkdir(parents=True)
            deployment = deployment_dir / "single-node.toml"
            deployment.write_text('[external_service]\nrevision = "workspace"\n', encoding="utf-8")
            revision = "b" * 40
            export_template.materialize_workspace_revisions(output, revision)
            self.assertIn(revision, deployment.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
