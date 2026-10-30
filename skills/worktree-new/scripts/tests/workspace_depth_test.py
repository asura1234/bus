import json
import subprocess

from repo_profile_test import Fixture, load


class WorkspaceDepthTests(Fixture):
    def setUp(self):
        super().setUp()
        self.profile = load("repo_profile")

    def test_node_workspace_glob_does_not_cover_deeper_independent_product(self):
        self.write(
            "package.json",
            json.dumps(
                {
                    "workspaces": ["packages/*"],
                    "scripts": {"build": "echo built", "test": "echo tested"},
                }
            ),
        )
        self.write("package-lock.json", "{}")
        self.write("packages/ui/package.json", '{"name":"ui"}')
        self.write(
            "packages/ui/tools/cli/package.json",
            '{"name":"independent-cli","scripts":{"build":"echo built","test":"echo tested"}}',
        )
        self.write("packages/ui/tools/cli/package-lock.json", "{}")
        self.commit()

        recipe = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertTrue(
            any("packages/ui/tools/cli/package.json" in item for item in recipe["unresolved"]),
            recipe,
        )

    def test_independent_cargo_product_inside_submodule_is_not_skipped(self):
        self.write("Cargo.toml", '[package]\nname="app"\nversion="0.1.0"\n')
        self.write("Cargo.lock", "version = 4\n")
        self.commit()
        source = self.root / "native-source"
        source.mkdir()
        subprocess.check_call(["git", "-C", str(source), "init", "-b", "main"], stdout=subprocess.DEVNULL)
        subprocess.check_call(["git", "-C", str(source), "config", "user.email", "fixture@example.test"])
        subprocess.check_call(["git", "-C", str(source), "config", "user.name", "Fixture"])
        (source / "Cargo.toml").write_text('[package]\nname="native"\nversion="0.1.0"\n')
        (source / "Cargo.lock").write_text("version = 4\n")
        subprocess.check_call(["git", "-C", str(source), "add", "."])
        subprocess.check_call(["git", "-C", str(source), "commit", "-qm", "native"])
        self.git("-c", "protocol.file.allow=always", "submodule", "add", str(source), "external/native")
        self.commit()
        recipe = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertIn(
            {"cwd": "external/native", "argv": ["cargo", "test", "--locked"]},
            recipe["smoke"],
        )

    def test_cargo_workspace_star_does_not_cover_deeper_product(self):
        self.write("Cargo.toml", '[workspace]\nmembers=["crates/*"]\nresolver="2"\n')
        self.write("Cargo.lock", "version = 4\n")
        self.write(
            "crates/ui/Cargo.toml", '[package]\nname="ui"\nversion="0.1.0"\n'
        )
        self.write(
            "crates/ui/tools/cli/Cargo.toml",
            '[package]\nname="cli"\nversion="0.1.0"\n',
        )
        self.write("crates/ui/tools/cli/Cargo.lock", "version = 4\n")
        self.commit()

        recipe = self.profile.scan(self.repo, use_cache=False)["recipe"]
        self.assertEqual(
            recipe["target_paths"], ["target", "crates/ui/tools/cli/target"]
        )
