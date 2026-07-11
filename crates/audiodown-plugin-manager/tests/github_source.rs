use audiodown_plugin_manager::github::GitHubRepositoryRef;

#[test]
fn accepts_only_canonical_public_repository_urls() {
    let source =
        GitHubRepositoryRef::parse("https://github.com/example-owner/example-repository").unwrap();
    assert_eq!(source.owner(), "example-owner");
    assert_eq!(source.repository(), "example-repository");
    assert_eq!(
        source.canonical_url(),
        "https://github.com/example-owner/example-repository"
    );
}

#[test]
fn normalizes_trailing_slashes_and_git_suffixes() {
    for value in [
        "https://github.com/example-owner/example-repository/",
        "https://github.com/example-owner/example-repository.git",
        "https://github.com/example-owner/example-repository.git/",
    ] {
        let source = GitHubRepositoryRef::parse(value).unwrap();
        assert_eq!(
            source.canonical_url(),
            "https://github.com/example-owner/example-repository"
        );
    }
}

#[test]
fn rejects_tokens_subpaths_queries_fragments_and_other_hosts() {
    for value in [
        "http://github.com/owner/repo",
        "https://user:token@github.com/owner/repo",
        "https://github.com:443/owner/repo",
        "https://github.com/owner/repo/tree/main",
        "https://github.com/owner/repo?tab=readme",
        "https://github.com/owner/repo#readme",
        "https://api.github.com/repos/owner/repo",
        "https://example.invalid/owner/repo",
    ] {
        assert!(GitHubRepositoryRef::parse(value).is_err(), "{value}");
    }
}
