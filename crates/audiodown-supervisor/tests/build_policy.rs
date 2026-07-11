#[path = "../src/builder.rs"]
mod builder;
#[path = "../src/trusted_images.rs"]
mod trusted_images;

use std::{collections::HashMap, path::Path};

use builder::{
    assembler_policy, build_networks, builder_policy, final_image_labels, managed_image_tag,
    normalize_build_output, npm_ci_command, proxy_policy, BuildConcurrency, BuildLog,
    BuildOutputEntry, BuildOutputEntryKind, BuildPolicyError, FinalImageMetadata,
    BUILD_LOG_LIMIT_BYTES,
};
use trusted_images::{
    pinned_base_reference, trusted_image_labels, verify_repo_digests, verify_trusted_image_labels,
    NodeImageLock, TrustedImageKind, BUILDER_IMAGE, NODE22_BASE_TAG, POLICY_VERSION, RUNTIME_IMAGE,
};

const LOCK_JSON: &str = include_str!("../../../docker/plugin-runtime/node22.lock.json");
const BUILDER_DOCKERFILE: &str =
    include_str!("../../../docker/plugin-runtime/node22-builder.Dockerfile");
const RUNTIME_DOCKERFILE: &str =
    include_str!("../../../docker/plugin-runtime/node22-runtime.Dockerfile");
const BUILD_RUNNER: &str = include_str!("../../../docker/plugin-runtime/node22-build-runner.js");
const SUPERVISOR_DOCKERFILE: &str = include_str!("../../../docker/supervisor.Dockerfile");

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SOURCE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MANIFEST_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SDK_HASH: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const ASSET_HASH: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

#[test]
fn node_base_is_locked_to_an_exact_digest_and_verified_after_pull() {
    let lock = NodeImageLock::parse(LOCK_JSON).unwrap();
    assert_eq!(lock.image, NODE22_BASE_TAG);
    assert!(lock.digest.starts_with("sha256:"));
    assert_eq!(lock.digest.len(), 71);
    assert!(lock.digest[7..]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));

    let pinned = pinned_base_reference(&lock).unwrap();
    assert_eq!(pinned, format!("{}@{}", lock.image, lock.digest));
    verify_repo_digests(&lock, &[pinned.clone()]).unwrap();
    assert!(verify_repo_digests(&lock, &[lock.image.clone()]).is_err());
    assert!(verify_repo_digests(
        &lock,
        &["node:22-bookworm-slim@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string()]
    )
    .is_err());
}

#[test]
fn trusted_images_have_fixed_names_and_attested_labels() {
    let lock = NodeImageLock::parse(LOCK_JSON).unwrap();
    assert_eq!(BUILDER_IMAGE, "audiodown/plugin-builder-node22:1.0");
    assert_eq!(RUNTIME_IMAGE, "audiodown/plugin-runtime-node22:1.0");

    for kind in [TrustedImageKind::Builder, TrustedImageKind::Runtime] {
        let labels = trusted_image_labels(kind, &lock.digest, SDK_HASH, ASSET_HASH);
        assert_eq!(labels["io.audiodown.base-image-digest"], lock.digest);
        assert_eq!(labels["io.audiodown.sdk-hash"], SDK_HASH);
        assert_eq!(labels["io.audiodown.asset-hash"], ASSET_HASH);
        assert_eq!(labels["io.audiodown.build-policy-version"], POLICY_VERSION);
        verify_trusted_image_labels(kind, &lock.digest, SDK_HASH, ASSET_HASH, &labels).unwrap();

        let mut tampered = labels.clone();
        tampered.insert(
            "io.audiodown.base-image-digest".into(),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        );
        assert!(
            verify_trusted_image_labels(kind, &lock.digest, SDK_HASH, ASSET_HASH, &tampered)
                .is_err()
        );

        let mut stale_assets = labels.clone();
        stale_assets.insert(
            "io.audiodown.asset-hash".into(),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into(),
        );
        assert!(verify_trusted_image_labels(
            kind,
            &lock.digest,
            SDK_HASH,
            ASSET_HASH,
            &stale_assets
        )
        .is_err());
    }
}

#[test]
fn build_concurrency_allows_one_global_build_and_one_operation_per_plugin() {
    let concurrency = BuildConcurrency::new();
    let plugin = concurrency
        .reserve_plugin("com.audiodown.virtual.content")
        .unwrap();
    assert!(matches!(
        concurrency.reserve_plugin("com.audiodown.virtual.content"),
        Err(BuildPolicyError::PluginBuildBusy)
    ));
    let other_plugin = concurrency
        .reserve_plugin("com.audiodown.virtual.credential")
        .unwrap();

    let global = concurrency.try_acquire_global().unwrap();
    assert!(matches!(
        concurrency.try_acquire_global(),
        Err(BuildPolicyError::GlobalBuildBusy)
    ));

    drop(global);
    let _global = concurrency.try_acquire_global().unwrap();
    drop(plugin);
    concurrency
        .reserve_plugin("com.audiodown.virtual.content")
        .unwrap();
    drop(other_plugin);
}

#[test]
fn managed_image_tag_is_deterministic_and_contains_only_short_hashes() {
    let tag = managed_image_tag("com.audiodown.virtual.content", COMMIT_SHA, SOURCE_HASH).unwrap();
    assert!(tag.starts_with("audiodown/plugin-"));
    let suffix = tag.strip_prefix("audiodown/plugin-").unwrap();
    let (plugin_hash, version) = suffix.split_once(':').unwrap();
    assert_eq!(plugin_hash.len(), 12);
    assert!(plugin_hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(version, "0123456789ab-bbbbbbbbbbbb");
}

#[test]
fn builder_and_proxy_have_fixed_network_identity_and_resource_limits() {
    let operation_id = "75de0d58-03f9-4db7-8a27-69ac7ddce8de";
    let networks = build_networks(operation_id);
    let builder = builder_policy(operation_id);
    let proxy = proxy_policy(operation_id);

    assert_eq!(
        networks.internal.name,
        format!("audiodown-build-{operation_id}-internal")
    );
    assert_eq!(networks.internal.driver, "bridge");
    assert!(networks.internal.internal);
    assert_eq!(
        networks.egress.name,
        format!("audiodown-build-{operation_id}-egress")
    );
    assert_eq!(networks.egress.driver, "bridge");
    assert!(!networks.egress.internal);
    assert_eq!(builder.networks, vec![networks.internal.name.clone()]);
    assert_eq!(
        proxy.networks,
        vec![networks.internal.name, networks.egress.name]
    );
    assert_eq!(
        builder.env["HTTPS_PROXY"],
        "http://audiodown-npm-proxy:18081"
    );
    assert_eq!(builder.user, "10001:10001");
    assert_eq!(builder.cap_drop, vec!["ALL"]);
    assert_eq!(builder.security_opt, vec!["no-new-privileges:true"]);
    assert!(builder.read_only_rootfs);
    assert_eq!(
        builder.tmpfs,
        HashMap::from([(
            "/workspace".to_string(),
            "rw,nosuid,nodev,size=268435456,uid=10001,gid=10001,mode=0700".to_string()
        )])
    );
    assert!(builder.bind_mounts.is_empty());
    assert!(builder.devices.is_empty());
    assert!(!builder.privileged);
    assert!(!builder.host_network);
    assert_eq!(builder.memory_bytes, 512 * 1024 * 1024);
    assert_eq!(builder.nano_cpus, 1_000_000_000);
    assert_eq!(builder.pids_limit, 128);
    assert_eq!(builder.timeout.as_secs(), 5 * 60);

    assert_eq!(proxy.user, "10002:10002");
    assert_eq!(proxy.network_aliases, vec!["audiodown-npm-proxy"]);
    assert_eq!(proxy.cap_drop, vec!["ALL"]);
    assert_eq!(proxy.security_opt, vec!["no-new-privileges:true"]);
    assert!(proxy.read_only_rootfs);
    assert!(proxy.bind_mounts.is_empty());
    assert!(proxy.devices.is_empty());
    assert!(!proxy.privileged);
    assert!(!proxy.host_network);
    assert_eq!(proxy.memory_bytes, 128 * 1024 * 1024);
    assert_eq!(proxy.nano_cpus, 500_000_000);
    assert_eq!(proxy.pids_limit, 64);
}

#[test]
fn npm_command_requires_an_explicit_validated_lifecycle_grant() {
    assert_eq!(
        npm_ci_command(false),
        [
            "npm",
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund"
        ]
    );
    assert_eq!(
        npm_ci_command(true),
        ["npm", "ci", "--omit=dev", "--no-audit", "--no-fund"]
    );
}

#[test]
fn build_log_limit_is_terminal_and_never_silently_truncates() {
    let mut log = BuildLog::default();
    log.push(&vec![b'x'; BUILD_LOG_LIMIT_BYTES]).unwrap();
    assert_eq!(log.as_bytes().len(), BUILD_LOG_LIMIT_BYTES);
    assert!(matches!(
        log.push(b"x"),
        Err(BuildPolicyError::BuildLogLimitExceeded)
    ));
    assert!(log.is_terminal());
    assert_eq!(log.terminal_code(), Some("BUILD_LOG_LIMIT_EXCEEDED"));
}

#[test]
fn assembler_is_never_started_and_has_network_disabled() {
    let assembler = assembler_policy();
    assert_eq!(assembler.image, RUNTIME_IMAGE);
    assert!(assembler.network_disabled);
    assert!(!assembler.start_container);
    assert!(!assembler.read_only_rootfs);
    assert!(assembler.bind_mounts.is_empty());
}

#[test]
fn build_output_rejects_unsafe_tar_entries() {
    for entry in [
        entry("/absolute.js", BuildOutputEntryKind::File, 1),
        entry("../escape.js", BuildOutputEntryKind::File, 1),
        entry("plugin/../../escape.js", BuildOutputEntryKind::File, 1),
        entry(
            "plugin/hard",
            BuildOutputEntryKind::HardLink {
                target: "plugin/index.js".into(),
            },
            0,
        ),
        entry("plugin/device", BuildOutputEntryKind::Device, 0),
        entry("plugin/pipe", BuildOutputEntryKind::Fifo, 0),
        entry("plugin/unknown", BuildOutputEntryKind::Other, 0),
        entry(
            "plugin/link",
            BuildOutputEntryKind::Symlink {
                target: "/etc/passwd".into(),
            },
            0,
        ),
        entry(
            "plugin/link",
            BuildOutputEntryKind::Symlink {
                target: "../../escape".into(),
            },
            0,
        ),
    ] {
        assert!(normalize_build_output(vec![entry]).is_err());
    }

    assert!(normalize_build_output(vec![
        entry("plugin/index.js", BuildOutputEntryKind::File, 1),
        entry("plugin/index.js", BuildOutputEntryKind::File, 1),
    ])
    .is_err());
}

#[test]
fn validated_output_is_repacked_with_normalized_root_owned_metadata() {
    let output = normalize_build_output(vec![
        entry("plugin", BuildOutputEntryKind::Directory, 0),
        entry("plugin/index.js", BuildOutputEntryKind::File, 42),
        entry(
            "plugin/current.js",
            BuildOutputEntryKind::Symlink {
                target: "index.js".into(),
            },
            0,
        ),
    ])
    .unwrap();

    assert_eq!(
        output
            .iter()
            .find(|item| item.path == Path::new("plugin"))
            .unwrap()
            .mode,
        0o755
    );
    assert_eq!(
        output
            .iter()
            .find(|item| item.path == Path::new("plugin/index.js"))
            .unwrap()
            .mode,
        0o644
    );
    assert_eq!(
        output
            .iter()
            .find(|item| item.path == Path::new("plugin/current.js"))
            .unwrap()
            .mode,
        0o777
    );
    for item in output {
        assert_eq!(item.uid, 0);
        assert_eq!(item.gid, 0);
        assert_eq!(item.mtime, 0);
        assert!(item.extended_metadata.is_empty());
    }
}

#[test]
fn final_image_labels_bind_every_installation_artifact() {
    let lock = NodeImageLock::parse(LOCK_JSON).unwrap();
    let metadata = FinalImageMetadata {
        installation_id: "installation-1",
        plugin_id: "com.audiodown.virtual.content",
        commit_sha: COMMIT_SHA,
        source_hash: SOURCE_HASH,
        manifest_hash: MANIFEST_HASH,
        base_image_digest: &lock.digest,
        sdk_hash: SDK_HASH,
    };
    let labels = final_image_labels(metadata).unwrap();
    assert_eq!(labels["io.audiodown.managed"], "true");
    assert_eq!(labels["io.audiodown.installation"], "installation-1");
    assert_eq!(
        labels["io.audiodown.plugin-id"],
        "com.audiodown.virtual.content"
    );
    assert_eq!(labels["io.audiodown.commit-sha"], COMMIT_SHA);
    assert_eq!(labels["io.audiodown.source-hash"], SOURCE_HASH);
    assert_eq!(labels["io.audiodown.manifest-hash"], MANIFEST_HASH);
    assert_eq!(labels["io.audiodown.base-image-digest"], lock.digest);
    assert_eq!(labels["io.audiodown.sdk-hash"], SDK_HASH);
}

#[test]
fn fixed_docker_assets_do_not_accept_untrusted_build_context_or_entrypoints() {
    let lock = NodeImageLock::embedded().unwrap();
    let pinned = format!("{}@{}", lock.image, lock.digest);
    assert!(BUILDER_DOCKERFILE.contains(&format!("FROM {pinned}")));
    assert!(RUNTIME_DOCKERFILE.contains(&format!("FROM {pinned}")));
    assert!(!BUILDER_DOCKERFILE.contains("COPY ."));
    assert!(!RUNTIME_DOCKERFILE.contains("COPY ."));
    assert!(BUILDER_DOCKERFILE.contains("node22-build-runner.js"));
    assert!(RUNTIME_DOCKERFILE.contains("plugin-sdk/node/"));
    assert!(BUILD_RUNNER.contains("--ignore-scripts"));
    assert!(!BUILD_RUNNER.contains("npm run"));
    assert!(!BUILD_RUNNER.contains("require("));
    assert!(!BUILD_RUNNER.contains("import("));

    for asset in [
        "node22.lock.json",
        "node22-builder.Dockerfile",
        "node22-runtime.Dockerfile",
        "node22-build-runner.js",
    ] {
        assert!(
            SUPERVISOR_DOCKERFILE.contains(asset),
            "Supervisor image must contain {asset}"
        );
    }
    assert!(SUPERVISOR_DOCKERFILE.contains("plugin-sdk/node/"));
}

fn entry(path: &str, kind: BuildOutputEntryKind, size: u64) -> BuildOutputEntry {
    BuildOutputEntry {
        path: Path::new(path).to_path_buf(),
        kind,
        size,
    }
}
