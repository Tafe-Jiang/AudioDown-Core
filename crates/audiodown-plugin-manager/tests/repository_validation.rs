use std::{
    fs,
    path::{Path, PathBuf},
};

use audiodown_plugin_manager::{archive::SnapshotLimits, validation::validate_repository};
use serde_json::{json, Value};
use tempfile::TempDir;

const VALID_INTEGRITY: &str = "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==";
const PLUGIN_PATH: &str = "plugins/virtual-content";

#[test]
fn validates_a_dependency_free_repository() {
    let fixture = RepositoryFixture::new();

    let validated = validate_repository(
        fixture.root(),
        &"1.0.0-alpha.1".parse().unwrap(),
        &"1.0.0".parse().unwrap(),
        SnapshotLimits::default(),
    )
    .unwrap();

    assert_eq!(validated.repository_id, "example.plugins");
    assert_eq!(validated.plugins.len(), 1);
    assert_eq!(
        validated.plugins[0].manifest.id.as_str(),
        "com.audiodown.virtual.content"
    );
    assert_eq!(validated.plugins[0].entry_path, "src/index.js");
    assert_eq!(validated.plugins[0].manifest_hash.len(), 64);
    assert_eq!(validated.plugins[0].source_hash.len(), 64);
}

#[test]
fn validates_registry_dependencies_and_declared_lifecycle_scripts() {
    let fixture = RepositoryFixture::new();
    fixture.add_dependency("1.0.0", valid_resolved_url(), Some(VALID_INTEGRITY));
    fixture.mutate_manifest(|manifest| {
        manifest["build"]["npmLifecycleScripts"] = json!({
            "required": true,
            "reason": "Generate a deterministic local file"
        });
        manifest["network"]["allowedHosts"] =
            json!(["api.virtual.invalid", "*.cdn.virtual.invalid"]);
    });
    fixture.mutate_package(|package| {
        package["scripts"] = json!({"install": "node scripts/install.js"});
    });

    let validated = validate_repository(
        fixture.root(),
        &"1.0.0-alpha.1".parse().unwrap(),
        &"1.0.0".parse().unwrap(),
        SnapshotLimits::default(),
    )
    .unwrap();

    assert!(validated.plugins[0].requires_lifecycle_scripts);
    assert_eq!(
        validated.plugins[0].lifecycle_script_reason.as_deref(),
        Some("Generate a deterministic local file")
    );
}

#[test]
fn hashes_source_files_in_stable_path_order() {
    let first = RepositoryFixture::new();
    first.write_file(&format!("{PLUGIN_PATH}/z.txt"), b"z");
    first.write_file(&format!("{PLUGIN_PATH}/a.txt"), b"a");

    let second = RepositoryFixture::new();
    second.write_file(&format!("{PLUGIN_PATH}/a.txt"), b"a");
    second.write_file(&format!("{PLUGIN_PATH}/z.txt"), b"z");

    let first_hash = validated_source_hash(&first);
    let second_hash = validated_source_hash(&second);
    assert_eq!(first_hash, second_hash);

    second.write_file(&format!("{PLUGIN_PATH}/a.txt"), b"changed");
    assert_ne!(first_hash, validated_source_hash(&second));
}

#[test]
fn rejects_unknown_schema_versions() {
    let repository = RepositoryFixture::new();
    repository.mutate_repository(|index| index["schemaVersion"] = json!("2.0"));
    assert_rejected(&repository, "repository schema version");

    let manifest = RepositoryFixture::new();
    manifest.mutate_manifest(|value| value["schemaVersion"] = json!("2.0"));
    assert_rejected(&manifest, "manifest schema version");
}

#[test]
fn rejects_repository_ids_outside_the_allowed_ascii_set() {
    for id in [
        "",
        "Example.plugins",
        "example plugins",
        "example/plugins",
        "example:plugins",
        "示例.plugins",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_repository(|index| index["repository"]["id"] = json!(id));
        assert_rejected(&fixture, id);
    }
}

#[test]
fn rejects_empty_or_overlong_repository_names() {
    for name in ["", "   ", &"x".repeat(121)] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_repository(|index| index["repository"]["name"] = json!(name));
        assert_rejected(&fixture, "repository name");
    }
}

#[test]
fn rejects_empty_or_duplicate_plugin_paths() {
    for path in ["", "   "] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_repository(|index| index["plugins"][0]["path"] = json!(path));
        assert_rejected(&fixture, "empty plugin path");
    }

    let fixture = RepositoryFixture::new();
    fixture.mutate_repository(|index| {
        index["plugins"] = json!([
            {"path": PLUGIN_PATH},
            {"path": PLUGIN_PATH}
        ]);
    });
    assert_rejected(&fixture, "duplicate plugin path");
}

#[test]
fn rejects_parent_or_absolute_plugin_paths() {
    for path in [
        "../outside",
        "plugins/../virtual-content",
        "/absolute/plugin",
        r"plugins\virtual-content",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_repository(|index| index["plugins"][0]["path"] = json!(path));
        assert_rejected(&fixture, path);
    }
}

#[test]
fn rejects_more_than_thirty_two_plugins() {
    let fixture = RepositoryFixture::new();
    let mut plugins = vec![json!({"path": PLUGIN_PATH})];

    for index in 1..33 {
        let relative_path = format!("plugins/virtual-{index:02}");
        fixture.write_plugin(
            &relative_path,
            &format!("com.audiodown.virtual.content{index:02}"),
            &format!("audiodown-virtual-content-{index:02}"),
        );
        plugins.push(json!({"path": relative_path}));
    }

    fixture.mutate_repository(|repository| repository["plugins"] = Value::Array(plugins));
    assert_rejected(&fixture, "more than 32 plugins");
}

#[test]
fn rejects_duplicate_manifest_ids() {
    let fixture = RepositoryFixture::new();
    fixture.write_plugin(
        "plugins/duplicate-id",
        "com.audiodown.virtual.content",
        "audiodown-duplicate-content",
    );
    fixture.mutate_repository(|repository| {
        repository["plugins"] = json!([
            {"path": PLUGIN_PATH},
            {"path": "plugins/duplicate-id"}
        ]);
    });

    assert_rejected(&fixture, "duplicate manifest id");
}

#[test]
fn rejects_index_paths_without_a_plugin_manifest_directory() {
    let missing = RepositoryFixture::new();
    missing.mutate_repository(|repository| {
        repository["plugins"][0]["path"] = json!("plugins/missing");
    });
    assert_rejected(&missing, "missing plugin directory");

    let file = RepositoryFixture::new();
    file.write_file("plugins/not-a-directory", b"not a directory");
    file.mutate_repository(|repository| {
        repository["plugins"][0]["path"] = json!("plugins/not-a-directory");
    });
    assert_rejected(&file, "plugin path is a file");

    let manifest = RepositoryFixture::new();
    fs::create_dir_all(manifest.root().join("plugins/no-manifest")).unwrap();
    manifest.mutate_repository(|repository| {
        repository["plugins"][0]["path"] = json!("plugins/no-manifest");
    });
    assert_rejected(&manifest, "plugin manifest is missing");
}

#[test]
fn rejects_node_runtimes_other_than_version_twenty_two() {
    let fixture = RepositoryFixture::new();
    fixture.mutate_manifest(|manifest| manifest["runtime"]["version"] = json!("20"));
    assert_rejected(&fixture, "Node.js runtime version");
}

#[test]
fn rejects_absolute_or_parent_directory_entry_paths() {
    for entry in [
        "/src/index.js",
        "../index.js",
        "src/../index.js",
        r"src\index.js",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_manifest(|manifest| manifest["runtime"]["entry"] = json!(entry));
        assert_rejected(&fixture, entry);
    }
}

#[test]
fn rejects_missing_entry_files() {
    let fixture = RepositoryFixture::new();
    fixture.mutate_manifest(|manifest| {
        manifest["runtime"]["entry"] = json!("src/missing.js");
    });
    assert_rejected(&fixture, "missing entry file");
}

#[test]
fn rejects_invalid_core_or_plugin_api_version_requirements() {
    for field in ["core", "pluginApi"] {
        for requirement in ["", "not-a-version-requirement"] {
            let fixture = RepositoryFixture::new();
            fixture.mutate_manifest(|manifest| {
                manifest["compatibility"][field] = json!(requirement);
            });
            assert_rejected(&fixture, field);
        }
    }
}

#[test]
fn rejects_incompatible_core_or_plugin_api_major_versions() {
    for field in ["core", "pluginApi"] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_manifest(|manifest| {
            manifest["compatibility"][field] = json!(">=2.0 <3.0");
        });
        assert_rejected(&fixture, field);
    }
}

#[test]
fn rejects_malformed_or_duplicate_capabilities() {
    for capabilities in [
        json!(["content"]),
        json!(["Content.search"]),
        json!(["content..search"]),
        json!(["content.search", "content.search"]),
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_manifest(|manifest| manifest["capabilities"] = capabilities);
        assert_rejected(&fixture, "capability");
    }
}

#[test]
fn rejects_invalid_allowed_hosts() {
    for host in [
        "",
        "API.virtual.invalid",
        "127.0.0.1",
        "::1",
        "localhost",
        "*.localhost",
        "*virtual.invalid",
        "api.*.virtual.invalid",
        "**.virtual.invalid",
        "api_virtual.invalid",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_manifest(|manifest| {
            manifest["network"]["allowedHosts"] = json!([host]);
        });
        assert_rejected(&fixture, host);
    }
}

#[test]
fn rejects_lifecycle_script_requirements_without_a_short_nonempty_reason() {
    for reason in [Value::Null, json!(""), json!("   "), json!("x".repeat(241))] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_manifest(|manifest| {
            manifest["build"]["npmLifecycleScripts"] = json!({
                "required": true,
                "reason": reason
            });
        });
        fixture.mutate_package(|package| {
            package["scripts"] = json!({"install": "node scripts/install.js"});
        });
        assert_rejected(&fixture, "lifecycle script reason");
    }
}

#[test]
fn rejects_lifecycle_scripts_when_the_manifest_does_not_require_them() {
    for script in [
        "preinstall",
        "install",
        "postinstall",
        "prepublish",
        "preprepare",
        "prepare",
        "postprepare",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.mutate_package(|package| {
            package["scripts"] = json!({script: "node scripts/lifecycle.js"});
        });
        assert_rejected(&fixture, script);
    }
}

#[test]
fn rejects_required_lifecycle_scripts_when_package_json_has_none() {
    let fixture = RepositoryFixture::new();
    fixture.mutate_manifest(|manifest| {
        manifest["build"]["npmLifecycleScripts"] = json!({
            "required": true,
            "reason": "Generate a deterministic local file"
        });
    });

    assert_rejected(&fixture, "required lifecycle script is absent");
}

#[test]
fn rejects_missing_package_json_or_package_lock_json() {
    for file in ["package.json", "package-lock.json"] {
        let fixture = RepositoryFixture::new();
        fs::remove_file(fixture.plugin_root().join(file)).unwrap();
        assert_rejected(&fixture, file);
    }
}

#[test]
fn rejects_lockfile_versions_below_two() {
    let fixture = RepositoryFixture::new();
    fixture.mutate_lockfile(|lockfile| lockfile["lockfileVersion"] = json!(1));
    assert_rejected(&fixture, "lockfileVersion 1");
}

#[test]
fn rejects_more_than_two_hundred_fifty_six_locked_packages() {
    let fixture = RepositoryFixture::new();
    let mut dependencies = serde_json::Map::new();
    let mut packages = fixture.read_lockfile()["packages"]
        .as_object()
        .unwrap()
        .clone();

    for index in 0..257 {
        let name = format!("virtual-dependency-{index:03}");
        dependencies.insert(name.clone(), json!("1.0.0"));
        packages.insert(
            format!("node_modules/{name}"),
            json!({
                "version": "1.0.0",
                "resolved": format!(
                    "https://registry.npmjs.org/{name}/-/{name}-1.0.0.tgz"
                ),
                "integrity": VALID_INTEGRITY
            }),
        );
    }

    fixture.mutate_package(|package| {
        package["dependencies"] = Value::Object(dependencies.clone());
    });
    fixture.mutate_lockfile(|lockfile| {
        lockfile["packages"][""]["dependencies"] = Value::Object(dependencies);
        lockfile["packages"] = Value::Object(packages);
    });

    assert_rejected(&fixture, "more than 256 locked packages");
}

#[test]
fn rejects_non_registry_dependency_specs_and_resolved_dependencies() {
    for specification in [
        "file:../dependency",
        "link:../dependency",
        "git+https://example.invalid/dependency.git",
        "github:owner/dependency",
        "owner/dependency",
        "workspace:*",
        "http://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz",
        "https://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz",
        "https://example.invalid/dependency.tgz",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.add_dependency(specification, valid_resolved_url(), Some(VALID_INTEGRITY));
        assert_rejected(&fixture, specification);
    }

    for resolved in [
        "file:../dependency",
        "link:../dependency",
        "git+https://example.invalid/dependency.git",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.add_dependency("1.0.0", resolved, Some(VALID_INTEGRITY));
        assert_rejected(&fixture, resolved);
    }
}

#[test]
fn rejects_unsafe_remote_package_resolved_urls() {
    for resolved in [
        "https://user:password@registry.npmjs.org/dependency/-/dependency-1.0.0.tgz",
        "https://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz?download=1",
        "https://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz#fragment",
        "http://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz",
        "https://packages.example.invalid/dependency-1.0.0.tgz",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.add_dependency("1.0.0", resolved, Some(VALID_INTEGRITY));
        assert_rejected(&fixture, resolved);
    }
}

#[test]
fn rejects_missing_or_invalid_sha512_integrity() {
    for integrity in [
        None,
        Some("sha256-YWJj"),
        Some("sha512-not-base64!"),
        Some("sha512-YWJj"),
    ] {
        let fixture = RepositoryFixture::new();
        fixture.add_dependency("1.0.0", valid_resolved_url(), integrity);
        assert_rejected(&fixture, "integrity");
    }
}

#[test]
fn rejects_forbidden_package_manager_and_container_files_anywhere() {
    for path in [
        ".npmrc",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "nested/.npmrc",
        "nested/npm-shrinkwrap.json",
        "nested/yarn.lock",
        "nested/pnpm-lock.yaml",
        "nested/package.json",
        "nested/package-lock.json",
        "Dockerfile",
        ".dockerignore",
        "nested/Dockerfile",
        "nested/.dockerignore",
    ] {
        let fixture = RepositoryFixture::new();
        fixture.write_file(&format!("{PLUGIN_PATH}/{path}"), b"forbidden");
        assert_rejected(&fixture, path);
    }
}

fn assert_rejected(fixture: &RepositoryFixture, case: &str) {
    let result = validate_repository(
        fixture.root(),
        &"1.0.0-alpha.1".parse().unwrap(),
        &"1.0.0".parse().unwrap(),
        SnapshotLimits::default(),
    );
    assert!(result.is_err(), "expected repository rejection for {case}");
}

fn validated_source_hash(fixture: &RepositoryFixture) -> String {
    validate_repository(
        fixture.root(),
        &"1.0.0-alpha.1".parse().unwrap(),
        &"1.0.0".parse().unwrap(),
        SnapshotLimits::default(),
    )
    .unwrap()
    .plugins
    .remove(0)
    .source_hash
}

fn valid_resolved_url() -> &'static str {
    "https://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz"
}

struct RepositoryFixture {
    _temp: TempDir,
    root: PathBuf,
}

impl RepositoryFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repository");
        fs::create_dir_all(&root).unwrap();

        let fixture = Self { _temp: temp, root };
        fixture.write_json(
            "audiodown-repository.json",
            &json!({
                "schemaVersion": "1.0",
                "repository": {
                    "id": "example.plugins",
                    "name": "Example Plugins"
                },
                "plugins": [
                    {"path": PLUGIN_PATH}
                ]
            }),
        );
        fixture.write_plugin(
            PLUGIN_PATH,
            "com.audiodown.virtual.content",
            "audiodown-virtual-content",
        );
        fixture
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn plugin_root(&self) -> PathBuf {
        self.root.join(PLUGIN_PATH)
    }

    fn write_plugin(&self, relative_path: &str, plugin_id: &str, package_name: &str) {
        let plugin_root = self.root.join(relative_path);
        fs::create_dir_all(plugin_root.join("src")).unwrap();
        fs::write(plugin_root.join("src/index.js"), b"export default {};\n").unwrap();
        write_json_at(
            &plugin_root.join("audiodown-plugin.json"),
            &json!({
                "schemaVersion": "1.0",
                "id": plugin_id,
                "name": "Virtual Content",
                "version": "1.0.0",
                "type": "content",
                "runtime": {
                    "type": "nodejs",
                    "version": "22",
                    "entry": "src/index.js"
                },
                "compatibility": {
                    "pluginApi": ">=1.0 <2.0",
                    "core": ">=1.0 <2.0"
                },
                "platform": {
                    "id": "virtual",
                    "name": "Virtual"
                },
                "capabilities": ["content.search"],
                "network": {
                    "allowedHosts": []
                },
                "build": {
                    "npmLifecycleScripts": {
                        "required": false
                    }
                }
            }),
        );
        write_json_at(
            &plugin_root.join("package.json"),
            &json!({
                "name": package_name,
                "version": "1.0.0",
                "private": true,
                "type": "module"
            }),
        );
        write_json_at(
            &plugin_root.join("package-lock.json"),
            &json!({
                "name": package_name,
                "version": "1.0.0",
                "lockfileVersion": 3,
                "requires": true,
                "packages": {
                    "": {
                        "name": package_name,
                        "version": "1.0.0"
                    }
                }
            }),
        );
    }

    fn write_file(&self, relative_path: &str, content: &[u8]) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn write_json(&self, relative_path: &str, value: &Value) {
        write_json_at(&self.root.join(relative_path), value);
    }

    fn mutate_repository(&self, mutate: impl FnOnce(&mut Value)) {
        self.mutate_json("audiodown-repository.json", mutate);
    }

    fn mutate_manifest(&self, mutate: impl FnOnce(&mut Value)) {
        self.mutate_json(&format!("{PLUGIN_PATH}/audiodown-plugin.json"), mutate);
    }

    fn mutate_package(&self, mutate: impl FnOnce(&mut Value)) {
        self.mutate_json(&format!("{PLUGIN_PATH}/package.json"), mutate);
    }

    fn mutate_lockfile(&self, mutate: impl FnOnce(&mut Value)) {
        self.mutate_json(&format!("{PLUGIN_PATH}/package-lock.json"), mutate);
    }

    fn read_lockfile(&self) -> Value {
        read_json(&self.plugin_root().join("package-lock.json"))
    }

    fn mutate_json(&self, relative_path: &str, mutate: impl FnOnce(&mut Value)) {
        let path = self.root.join(relative_path);
        let mut value = read_json(&path);
        mutate(&mut value);
        write_json_at(&path, &value);
    }

    fn add_dependency(&self, specification: &str, resolved: &str, integrity: Option<&str>) {
        self.mutate_package(|package| {
            package["dependencies"] = json!({"dependency": specification});
        });
        self.mutate_lockfile(|lockfile| {
            lockfile["packages"][""]["dependencies"] = json!({
                "dependency": specification
            });
            let mut package = json!({
                "version": "1.0.0",
                "resolved": resolved
            });
            if let Some(integrity) = integrity {
                package["integrity"] = json!(integrity);
            }
            lockfile["packages"]["node_modules/dependency"] = package;
        });
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_json_at(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
