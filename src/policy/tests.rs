use super::*;

// ─── Unrestricted ─────────────────────────────

#[test]
fn unrestricted_allows_anything() {
    let policy = Unrestricted;
    let result = policy.resolve(Path::new("/any/path"), PathOp::Read);
    assert!(result.is_ok());
}

#[test]
fn unrestricted_preserves_path() {
    let policy = Unrestricted;
    let result = policy.resolve(Path::new("relative/path.txt"), PathOp::Write);
    assert_eq!(result.unwrap().display(), "relative/path.txt");
}

// ─── Sandboxed ────────────────────────────────

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_allows_within_root() {
    let dir = std::env::temp_dir();
    let policy = Sandboxed::new([&dir]).unwrap();
    let test_path = dir.join("test_file.txt");
    std::fs::write(&test_path, "").unwrap();
    let result = policy.resolve(&test_path, PathOp::Read);
    assert!(result.is_ok());
    let _ = std::fs::remove_file(&test_path);
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_denies_outside_root() {
    let sandbox = std::env::temp_dir().join("mlua_bat_sandbox_deny_test");
    std::fs::create_dir_all(&sandbox).unwrap();
    let policy = Sandboxed::new([&sandbox]).unwrap();
    let result = policy.resolve(Path::new("/usr"), PathOp::Read);
    assert!(result.is_err());
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_new_fails_for_nonexistent_root() {
    let result = Sandboxed::new(["/tmp/mlua_bat_sandbox_test_nonexistent_root_xyz"]);
    assert!(result.is_err());
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_new_rejects_empty_roots() {
    let result = Sandboxed::new(Vec::<PathBuf>::new());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("at least one root"),
        "error should explain the issue: {err}"
    );
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_read_only_denies_write() {
    let dir = std::env::temp_dir();
    let policy = Sandboxed::new([&dir]).unwrap().read_only();
    let test_path = dir.join("readonly_test.txt");
    std::fs::write(&test_path, "").unwrap();

    assert!(policy.resolve(&test_path, PathOp::Read).is_ok());
    assert!(policy.resolve(&test_path, PathOp::List).is_ok());
    assert!(policy.resolve(&test_path, PathOp::Write).is_err());
    assert!(policy.resolve(&test_path, PathOp::Delete).is_err());

    let _ = std::fs::remove_file(&test_path);
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_allows_new_file_under_root() {
    let dir = std::env::temp_dir();
    let policy = Sandboxed::new([&dir]).unwrap();
    let new_file = dir.join("mlua_bat_nonexistent_file_xyz.txt");
    let result = policy.resolve(&new_file, PathOp::Write);
    assert!(result.is_ok());
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_dotdot_traversal_blocked_by_cap_std() {
    let sandbox = std::env::temp_dir().join("mlua_bat_sandbox_dotdot_test");
    std::fs::create_dir_all(&sandbox).unwrap();
    let policy = Sandboxed::new([&sandbox]).unwrap();

    // Even if lexical normalization resolves ".." to a path outside,
    // cap_std's Dir operations will block the actual I/O.
    // The resolve step itself may or may not catch it
    // (it's a UX layer, not the security boundary).
    let traversal_path = sandbox.join("sub/../../etc/passwd");
    let result = policy.resolve(&traversal_path, PathOp::Read);
    // Either denied at resolve (lexical check) or would be denied by cap_std at I/O
    // For this test: lexical_clean resolves ".." → path goes outside sandbox → denied
    assert!(result.is_err());
    let _ = std::fs::remove_dir_all(&sandbox);
}

// ─── lexical_clean ────────────────────────────

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_absolute_basic() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_absolute_to_root() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("/a/..")), PathBuf::from("/"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_absolute_dotdot_at_root_clamped() {
    use sandbox::lexical_clean;
    // Can't go above root — ".." is silently dropped
    assert_eq!(lexical_clean(Path::new("/../")), PathBuf::from("/"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_multiple_dotdot() {
    use sandbox::lexical_clean;
    assert_eq!(
        lexical_clean(Path::new("/a/b/c/../../d")),
        PathBuf::from("/a/d")
    );
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_relative_preserves_leading_dotdot() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("../a/b")), PathBuf::from("../a/b"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_relative_resolves_inner_dotdot() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("a/b/../c")), PathBuf::from("a/c"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_dot_removed() {
    use sandbox::lexical_clean;
    assert_eq!(
        lexical_clean(Path::new("/a/./b/./c")),
        PathBuf::from("/a/b/c")
    );
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_empty_becomes_dot() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("")), PathBuf::from("."));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_only_dots() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("./././.")), PathBuf::from("."));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_consecutive_dotdot_relative() {
    use sandbox::lexical_clean;
    // ../../x — both ".." should be preserved (relative, no normal to pop)
    assert_eq!(
        lexical_clean(Path::new("../../x")),
        PathBuf::from("../../x")
    );
}

// ─── lexical_clean edge cases ─────────────────

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_absolute_deep_dotdot_clamped_at_root() {
    use sandbox::lexical_clean;
    // More ".." than depth — should clamp at root
    assert_eq!(
        lexical_clean(Path::new("/a/../../../b")),
        PathBuf::from("/b")
    );
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_relative_dotdot_then_normal_then_dotdot() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("../a/../b")), PathBuf::from("../b"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_trailing_slash_preserved_as_component() {
    use sandbox::lexical_clean;
    // "/a/b/" components: RootDir, Normal("a"), Normal("b")
    // (trailing slash is ignored by Path::components)
    assert_eq!(lexical_clean(Path::new("/a/b/")), PathBuf::from("/a/b"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_root_only() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("/")), PathBuf::from("/"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_relative_single_component() {
    use sandbox::lexical_clean;
    assert_eq!(lexical_clean(Path::new("foo")), PathBuf::from("foo"));
}

#[cfg(feature = "sandbox")]
#[test]
fn lexical_clean_dotdot_pops_last_not_earlier() {
    use sandbox::lexical_clean;
    // a/b/c/../.. → should pop c then b → result: a
    assert_eq!(lexical_clean(Path::new("a/b/c/../..")), PathBuf::from("a"));
}

// ─── Sandboxed cap_std enforcement ────────────

#[cfg(feature = "sandbox")]
#[test]
fn cap_std_blocks_dotdot_escape_at_io_level() {
    let sandbox = std::env::temp_dir().join("mlua_bat_cap_escape_test");
    std::fs::create_dir_all(sandbox.join("inner")).unwrap();
    std::fs::write(sandbox.join("inner/file.txt"), "inside").unwrap();

    let canonical = sandbox.canonicalize().unwrap();
    let dir = cap_std::fs::Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()).unwrap();

    // Reading "inner/file.txt" should succeed
    assert_eq!(dir.read_to_string("inner/file.txt").unwrap(), "inside");

    // Attempting ".." escape should be rejected by cap_std
    let result = dir.read_to_string("inner/../../etc/passwd");
    assert!(result.is_err());

    // Absolute path should also be rejected
    let result = dir.open("/etc/passwd");
    assert!(result.is_err());

    let _ = std::fs::remove_dir_all(&sandbox);
}

#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_multiple_roots() {
    let root_a = std::env::temp_dir().join("mlua_bat_multi_root_a");
    let root_b = std::env::temp_dir().join("mlua_bat_multi_root_b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();

    let policy = Sandboxed::new([&root_a, &root_b]).unwrap();

    // Both roots should be accessible
    let file_a = root_a.join("a.txt");
    std::fs::write(&file_a, "").unwrap();
    assert!(policy.resolve(&file_a, PathOp::Read).is_ok());

    let file_b = root_b.join("b.txt");
    std::fs::write(&file_b, "").unwrap();
    assert!(policy.resolve(&file_b, PathOp::Read).is_ok());

    // Outside both should fail
    assert!(policy.resolve(Path::new("/usr"), PathOp::Read).is_err());

    let _ = std::fs::remove_dir_all(&root_a);
    let _ = std::fs::remove_dir_all(&root_b);
}

// ─── FsAccess ─────────────────────────────────

#[test]
fn fs_access_direct_read_write() {
    let dir = std::env::temp_dir().join("mlua_bat_test_fsaccess");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.txt");

    let access = FsAccess::direct(&path);
    access.write(b"hello").unwrap();
    assert_eq!(access.read_to_string().unwrap(), "hello");
    assert!(access.exists());
    assert!(!access.is_dir());
    access.remove().unwrap();
    assert!(!access.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "sandbox")]
#[test]
fn fs_access_capped_read_write() {
    use std::sync::Arc;

    let dir = std::env::temp_dir().join("mlua_bat_test_fsaccess_capped");
    std::fs::create_dir_all(&dir).unwrap();

    let canonical = dir.canonicalize().unwrap();
    let cap_dir = Arc::new(
        cap_std::fs::Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()).unwrap(),
    );

    let access = FsAccess(FsAccessInner::Capped {
        dir: Arc::clone(&cap_dir),
        relative: PathBuf::from("capped_test.txt"),
    });

    access.write(b"cap hello").unwrap();
    assert_eq!(access.read_to_string().unwrap(), "cap hello");
    assert!(access.exists());
    access.remove().unwrap();
    assert!(!access.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── proptest ─────────────────────────────────

#[cfg(feature = "sandbox")]
mod proptest_suite {
    use super::*;
    use proptest::prelude::*;

    use crate::policy::sandbox::lexical_clean;

    /// Generate path-like strings composed of normal segments, ".", and ".."
    fn path_components() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![Just("..".to_string()), Just(".".to_string()), "[a-z]{1,8}",],
            1..=8,
        )
        .prop_map(|parts| parts.join("/"))
    }

    proptest! {
        /// lexical_clean must never panic, regardless of input
        #[test]
        fn lexical_clean_never_panics(s in "\\PC{0,200}") {
            let _ = lexical_clean(Path::new(&s));
        }

        /// lexical_clean result must never contain intermediate "." components.
        /// A standalone "." is the canonical representation of an empty relative path.
        #[test]
        fn lexical_clean_no_intermediate_curdir(path in path_components()) {
            let cleaned = lexical_clean(Path::new(&path));
            // "." as the sole result is the canonical empty-path form, not CurDir
            if cleaned.as_os_str() == "." {
                return Ok(());
            }
            for component in cleaned.components() {
                prop_assert!(
                    !matches!(component, std::path::Component::CurDir),
                    "lexical_clean output contains CurDir: {:?}",
                    cleaned
                );
            }
        }

        /// For absolute paths, lexical_clean must never produce ".." components
        #[test]
        fn lexical_clean_absolute_no_dotdot(path in path_components()) {
            let abs_path = format!("/{path}");
            let cleaned = lexical_clean(Path::new(&abs_path));
            for component in cleaned.components() {
                prop_assert!(
                    !matches!(component, std::path::Component::ParentDir),
                    "absolute cleaned path contains ParentDir: {:?} (input: {})",
                    cleaned,
                    abs_path
                );
            }
        }

        /// For absolute paths, lexical_clean output must start with root
        #[test]
        fn lexical_clean_absolute_starts_with_root(path in path_components()) {
            let abs_path = format!("/{path}");
            let cleaned = lexical_clean(Path::new(&abs_path));
            prop_assert!(
                cleaned.is_absolute(),
                "absolute input produced non-absolute output: {:?} (input: {})",
                cleaned,
                abs_path
            );
        }

        /// lexical_clean is idempotent: cleaning twice gives the same result
        #[test]
        fn lexical_clean_idempotent(path in path_components()) {
            let once = lexical_clean(Path::new(&path));
            let twice = lexical_clean(&once);
            prop_assert_eq!(
                &once, &twice,
                "not idempotent: once={:?}, twice={:?} (input: {})",
                once, twice, path
            );
        }

        /// Sandboxed policy: any path outside roots must be denied
        #[test]
        fn sandboxed_never_allows_outside(
            subdir in "[a-z]{1,6}",
            outside in "[a-z]{1,6}",
        ) {
            let sandbox = std::env::temp_dir().join(format!("mlua_prop_{subdir}"));
            std::fs::create_dir_all(&sandbox).unwrap();

            let policy = Sandboxed::new([&sandbox]).unwrap();

            // A path under /usr/<random> should always be denied
            let outside_path = PathBuf::from(format!("/usr/{outside}"));
            let result = policy.resolve(&outside_path, PathOp::Read);
            prop_assert!(
                result.is_err(),
                "path outside sandbox was allowed: {:?}",
                outside_path
            );

            let _ = std::fs::remove_dir_all(&sandbox);
        }
    }
}

// ─── symlink sandbox escape tests ────────────

/// Test that a symlink inside the sandbox pointing outside is blocked
/// at cap_std I/O level.
#[cfg(feature = "sandbox")]
#[test]
fn cap_std_blocks_symlink_escape() {
    let sandbox = std::env::temp_dir().join("mlua_bat_symlink_escape_test");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).unwrap();

    // Create a symlink inside sandbox pointing outside
    let link_path = sandbox.join("escape_link");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc", &link_path).unwrap();
    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_dir_all(&sandbox);
        return; // symlink tests only apply on unix
    }

    let canonical = sandbox.canonicalize().unwrap();
    let dir = cap_std::fs::Dir::open_ambient_dir(&canonical, cap_std::ambient_authority()).unwrap();

    // Attempting to read through the symlink should be blocked by cap_std
    let result = dir.read_to_string("escape_link/passwd");
    assert!(
        result.is_err(),
        "cap_std should block symlink escape: {:?}",
        result
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

/// Test that Sandboxed policy denies access via symlink pointing outside.
#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_blocks_symlink_to_outside() {
    let sandbox = std::env::temp_dir().join("mlua_bat_sandbox_symlink_test");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).unwrap();

    let link_path = sandbox.join("outside_link");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr", &link_path).unwrap();
    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_dir_all(&sandbox);
        return;
    }

    let policy = Sandboxed::new([&sandbox]).unwrap();

    // resolve sees the symlink target (/usr) via canonicalize —
    // it's outside the sandbox root, so resolve denies it.
    // Even if resolve somehow allowed it, cap_std would block I/O.
    let result = policy.resolve(&link_path.join("bin"), PathOp::Read);
    assert!(
        result.is_err(),
        "symlink pointing outside sandbox should be denied"
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

/// Test that Sandboxed policy allows symlink within the same sandbox.
#[cfg(feature = "sandbox")]
#[test]
fn sandboxed_allows_symlink_within_sandbox() {
    let sandbox = std::env::temp_dir().join("mlua_bat_sandbox_symlink_internal");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(sandbox.join("real_dir")).unwrap();
    std::fs::write(sandbox.join("real_dir/file.txt"), "data").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(sandbox.join("real_dir"), sandbox.join("link_dir")).unwrap();
    #[cfg(not(unix))]
    {
        let _ = std::fs::remove_dir_all(&sandbox);
        return;
    }

    let policy = Sandboxed::new([&sandbox]).unwrap();

    // Symlink within sandbox → canonicalize resolves to real_dir → still under root
    let result = policy.resolve(&sandbox.join("link_dir/file.txt"), PathOp::Read);
    assert!(
        result.is_ok(),
        "symlink within sandbox should be allowed: {:?}",
        result.err()
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

// ─── normalize_for_matching deep fallback ────

/// Test that normalize_for_matching handles deeply nested non-existent
/// paths under a real directory (e.g. /tmp/sandbox/a/b/c/d).
#[cfg(feature = "sandbox")]
#[test]
fn normalize_deep_nonexistent_resolves_symlinks() {
    let sandbox = std::env::temp_dir().join("mlua_bat_normalize_deep_test");
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).unwrap();

    let policy = Sandboxed::new([&sandbox]).unwrap();

    // a/b/c/d don't exist, but sandbox does.
    // normalize_for_matching should canonicalize sandbox (resolving
    // /tmp → /private/tmp on macOS) and append a/b/c/d.
    let deep_path = sandbox.join("a/b/c/d/file.txt");
    let result = policy.resolve(&deep_path, PathOp::Write);
    assert!(
        result.is_ok(),
        "deep non-existent path under sandbox should be allowed: {:?}",
        result.err()
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

/// Test that error messages include operation type.
#[cfg(feature = "sandbox")]
#[test]
fn error_message_includes_op_type() {
    let sandbox = std::env::temp_dir().join("mlua_bat_error_msg_test");
    std::fs::create_dir_all(&sandbox).unwrap();
    let policy = Sandboxed::new([&sandbox]).unwrap();

    let result = policy.resolve(Path::new("/usr/bin/ls"), PathOp::Read);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("read denied"),
        "error should mention operation type: {err}"
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

/// Test read_only error message includes operation type.
#[cfg(feature = "sandbox")]
#[test]
fn read_only_error_includes_op_type() {
    let sandbox = std::env::temp_dir().join("mlua_bat_readonly_msg_test");
    std::fs::create_dir_all(&sandbox).unwrap();
    let file = sandbox.join("file.txt");
    std::fs::write(&file, "").unwrap();

    let policy = Sandboxed::new([&sandbox]).unwrap().read_only();
    let result = policy.resolve(&file, PathOp::Write);
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("write denied"),
        "error should mention operation type: {err}"
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

// ─── HttpPolicy ──────────────────────────────

#[test]
fn http_unrestricted_allows_anything() {
    let policy = Unrestricted;
    assert!(HttpPolicy::check_url(&policy, "http://localhost:8080", "GET").is_ok());
    assert!(HttpPolicy::check_url(&policy, "https://example.com", "POST").is_ok());
}

#[test]
fn http_allowlist_allows_matching_host() {
    let policy = HttpAllowList::new(["api.example.com", "httpbin.org"]);
    assert!(policy
        .check_url("https://api.example.com/v1/data", "GET")
        .is_ok());
    assert!(policy.check_url("https://httpbin.org/post", "POST").is_ok());
}

#[test]
fn http_allowlist_denies_non_matching_host() {
    let policy = HttpAllowList::new(["api.example.com"]);
    let result = policy.check_url("https://evil.com/steal", "GET");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("GET denied"), "error: {err}");
    assert!(err.contains("does not match"), "error: {err}");
}

#[test]
fn http_allowlist_empty_denies_all() {
    let policy = HttpAllowList::new(Vec::<String>::new());
    assert!(policy.check_url("https://example.com", "GET").is_err());
}

// ─── C-1: Host-only matching security tests ──────

#[test]
fn http_allowlist_not_bypassed_by_query_string() {
    let policy = HttpAllowList::new(["example.com"]);
    // Query string should NOT match
    let result = policy.check_url("https://evil.com/?ref=example.com", "GET");
    assert!(result.is_err(), "query string bypass should be blocked");
}

#[test]
fn http_allowlist_not_bypassed_by_path() {
    let policy = HttpAllowList::new(["api.internal"]);
    // Path segment should NOT match
    let result = policy.check_url("https://evil.com/api.internal/path", "GET");
    assert!(result.is_err(), "path bypass should be blocked");
}

#[test]
fn http_allowlist_not_bypassed_by_fragment() {
    let policy = HttpAllowList::new(["example.com"]);
    let result = policy.check_url("https://evil.com/#example.com", "GET");
    assert!(result.is_err(), "fragment bypass should be blocked");
}

#[test]
fn http_allowlist_not_bypassed_by_userinfo() {
    let policy = HttpAllowList::new(["example.com"]);
    let result = policy.check_url("https://example.com@evil.com/path", "GET");
    assert!(result.is_err(), "userinfo bypass should be blocked");
}

#[test]
fn http_allowlist_not_bypassed_by_suffix_overlap() {
    let policy = HttpAllowList::new(["example.com"]);
    // "notexample.com" ends with "example.com" but is NOT a subdomain
    let result = policy.check_url("https://notexample.com/path", "GET");
    assert!(result.is_err(), "suffix overlap bypass should be blocked");
}

#[test]
fn http_allowlist_matches_subdomain() {
    let policy = HttpAllowList::new(["example.com"]);
    assert!(policy
        .check_url("https://api.example.com/v1", "GET")
        .is_ok());
    assert!(policy
        .check_url("https://deep.sub.example.com/", "GET")
        .is_ok());
}

#[test]
fn http_allowlist_strips_port() {
    let policy = HttpAllowList::new(["localhost"]);
    assert!(policy.check_url("http://localhost:8080/api", "GET").is_ok());
}

#[test]
fn http_allowlist_handles_ipv6() {
    let policy = HttpAllowList::new(["::1"]);
    assert!(policy.check_url("http://[::1]:8080/api", "GET").is_ok());
}

// ─── extract_url_host unit tests ──────

#[test]
fn extract_host_standard_url() {
    use super::http::extract_url_host;
    assert_eq!(
        extract_url_host("https://example.com/path"),
        Some("example.com")
    );
}

#[test]
fn extract_host_with_port() {
    use super::http::extract_url_host;
    assert_eq!(
        extract_url_host("http://localhost:8080/api"),
        Some("localhost")
    );
}

#[test]
fn extract_host_with_userinfo() {
    use super::http::extract_url_host;
    assert_eq!(
        extract_url_host("https://user:pass@example.com/path"),
        Some("example.com")
    );
}

#[test]
fn extract_host_ipv6() {
    use super::http::extract_url_host;
    assert_eq!(extract_url_host("http://[::1]:8080/api"), Some("::1"));
}

#[test]
fn extract_host_no_path() {
    use super::http::extract_url_host;
    assert_eq!(extract_url_host("https://example.com"), Some("example.com"));
}

#[test]
fn extract_host_with_query_only() {
    use super::http::extract_url_host;
    assert_eq!(
        extract_url_host("https://example.com?q=1"),
        Some("example.com")
    );
}

#[test]
fn extract_host_no_scheme() {
    use super::http::extract_url_host;
    assert_eq!(extract_url_host("example.com/path"), None);
}

#[cfg(feature = "http")]
#[test]
fn http_policy_integration() {
    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .http_policy(HttpAllowList::new(["httpbin.org"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    // Allowed host — function exists and would try to connect
    // (we can't test actual HTTP here, just that the policy check passes
    // before the connection error)
    let result: mlua::Result<mlua::Value> = lua
        .load(r#"return std.http.get("https://blocked.example.com/test")"#)
        .eval();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("does not match any allowed host"),
        "should be policy denial, got: {err_msg}"
    );
}

// ─── EnvPolicy ───────────────────────────────

#[test]
fn env_unrestricted_allows_anything() {
    let policy = Unrestricted;
    assert!(EnvPolicy::check_get(&policy, "SECRET_KEY").is_ok());
    assert!(EnvPolicy::check_set(&policy, "SECRET_KEY").is_ok());
}

#[test]
fn env_allowlist_allows_listed_keys() {
    let policy = EnvAllowList::new(["HOME", "PATH", "LANG"]);
    assert!(policy.check_get("HOME").is_ok());
    assert!(policy.check_get("PATH").is_ok());
    assert!(policy.check_set("HOME").is_ok());
}

#[test]
fn env_allowlist_denies_unlisted_keys() {
    let policy = EnvAllowList::new(["HOME", "PATH"]);
    let result = policy.check_get("AWS_SECRET_ACCESS_KEY");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("read denied"), "error: {err}");
    assert!(err.contains("not in the allow list"), "error: {err}");
}

#[test]
fn env_allowlist_read_only_denies_set() {
    let policy = EnvAllowList::new(["HOME"]).read_only();
    assert!(policy.check_get("HOME").is_ok());
    let result = policy.check_set("HOME");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("read-only"), "error: {err}");
}

#[test]
fn env_allowlist_empty_denies_all() {
    let policy = EnvAllowList::new(Vec::<String>::new());
    assert!(policy.check_get("HOME").is_err());
    assert!(policy.check_set("HOME").is_err());
}

#[test]
fn env_policy_integration_get_blocked() {
    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .env_policy(EnvAllowList::new(["ALLOWED_VAR"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.get("SECRET_KEY")"#).eval();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not in the allow list"),
        "should be policy denial, got: {err_msg}"
    );
}

#[test]
fn env_policy_integration_get_allowed() {
    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .env_policy(EnvAllowList::new(["HOME"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    // HOME is in allow list — should succeed (returns value or nil)
    let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.get("HOME")"#).eval();
    assert!(result.is_ok());
}

#[test]
fn env_policy_integration_set_blocked() {
    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .env_policy(EnvAllowList::new(["HOME"]).read_only())
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    let result: mlua::Result<mlua::Value> = lua.load(r#"std.env.set("HOME", "hacked")"#).eval();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("read-only"),
        "should be read-only denial, got: {err_msg}"
    );
}

#[test]
fn env_policy_integration_home_blocked() {
    let lua = mlua::Lua::new();
    let config = crate::config::Config::builder()
        .env_policy(EnvAllowList::new(["PATH"]))
        .build()
        .unwrap();
    crate::register_all_with(&lua, "std", config).unwrap();

    // HOME is not in allow list
    let result: mlua::Result<mlua::Value> = lua.load(r#"return std.env.home()"#).eval();
    assert!(result.is_err());
}

// ─── LlmPolicy ──────────────────────────────

#[test]
fn llm_unrestricted_allows_anything() {
    let policy = Unrestricted;
    assert!(
        LlmPolicy::check_request(&policy, "openai", "gpt-4o", "https://api.openai.com").is_ok()
    );
    assert!(
        LlmPolicy::check_request(&policy, "custom", "my-model", "http://localhost:8080").is_ok()
    );
}

#[test]
fn llm_allowlist_allows_listed_providers() {
    let policy = LlmAllowList::new(["ollama", "openai"]);
    assert!(policy
        .check_request("ollama", "llama3.2", "http://localhost:11434")
        .is_ok());
    assert!(policy
        .check_request("openai", "gpt-4o", "https://api.openai.com")
        .is_ok());
}

#[test]
fn llm_allowlist_denies_unlisted_providers() {
    let policy = LlmAllowList::new(["ollama"]);
    let result = policy.check_request("openai", "gpt-4o", "https://api.openai.com");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("LLM denied"), "error: {err}");
    assert!(err.contains("not in the allow list"), "error: {err}");
}

#[test]
fn llm_allowlist_empty_denies_all() {
    let policy = LlmAllowList::new(Vec::<String>::new());
    assert!(policy
        .check_request("openai", "gpt-4o", "https://api.openai.com")
        .is_err());
}
