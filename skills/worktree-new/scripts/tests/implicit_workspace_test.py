from repo_profile_test import Fixture, load


class ImplicitWorkspaceTests(Fixture):
    def test_implicit_path_dependency_member_is_tested(self):
        self.write("Cargo.toml", '[package]\nname="app"\nversion="0.1.0"\nedition="2021"\n'
                   '[dependencies]\nnative={path="crates/native"}\n[workspace]\nmembers=[]\n')
        self.write("Cargo.lock", "version = 4\n")
        self.write("src/lib.rs", "")
        self.write("crates/native/Cargo.toml", '[package]\nname="native"\nversion="0.1.0"\nedition="2021"\n')
        self.write("crates/native/src/lib.rs", "")
        self.commit()

        recipe = load("repo_profile").scan(self.repo, use_cache=False)["recipe"]
        self.assertTrue(any("--workspace" in c["argv"] or c["cwd"] == "crates/native"
                            for c in recipe["smoke"] if "test" in c["argv"]), recipe)

    def test_implicit_path_member_is_included_in_smoke(self):
        self.write("Cargo.toml", '[package]\nname="app"\nversion="0.1.0"\nedition="2021"\n'
                   '[workspace]\nresolver="2"\n[dependencies]\nnative={path="external/native"}\n')
        self.write("src/lib.rs", "pub fn root() {}\n")
        self.write("external/native/Cargo.toml", '[package]\nname="native"\nversion="0.1.0"\nedition="2021"\n')
        self.write("external/native/src/lib.rs", '#[test]\nfn member_test() { panic!("member test ran"); }\n')
        self.write("Cargo.lock", 'version = 4\n\n[[package]]\nname = "app"\nversion = "0.1.0"\n'
                   'dependencies = [\n "native",\n]\n\n[[package]]\nname = "native"\nversion = "0.1.0"\n')
        self.commit()

        recipe = load("repo_profile").scan(self.repo, use_cache=False)["recipe"]
        test_commands = [c for c in recipe["smoke"] if c["argv"][:2] == ["cargo", "test"]]
        self.assertTrue(any("--workspace" in c["argv"] or c["cwd"] == "external/native"
                            for c in test_commands), recipe)
