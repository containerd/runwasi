use std::path::PathBuf;
use std::time::Duration;

use containerd_shim_wasm::shim::cache::CacheConfig;

const DIR: &str = "RUNWASI_MODULE_CACHE_DIR";
const TTL: &str = "RUNWASI_MODULE_CACHE_TTL_SECS";
const MAX_BYTES: &str = "RUNWASI_MODULE_CACHE_MAX_BYTES";

const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024;

const UNPARSABLE: [&str; 5] = ["", "90s", "-1", "1e6", "18446744073709551616"];

fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let mut all = vec![(DIR, None), (TTL, None), (MAX_BYTES, None)];
    for (name, value) in vars {
        all.retain(|(existing, _)| existing != name);
        all.push((name, *value));
    }
    temp_env::with_vars(all, f);
}

#[test]
fn the_cache_is_disabled_without_a_directory() {
    with_env(&[(TTL, Some("60")), (MAX_BYTES, Some("4096"))], || {
        assert!(CacheConfig::from_env().is_none());
    });
}

#[test]
fn an_empty_directory_leaves_the_cache_disabled() {
    with_env(&[(DIR, Some(""))], || {
        assert!(CacheConfig::from_env().is_none());
    });
}

#[test]
fn a_directory_enables_the_cache() {
    with_env(&[(DIR, Some("/var/cache/runwasi"))], || {
        let config = CacheConfig::from_env().expect("the cache should be enabled");
        assert_eq!(config.dir, PathBuf::from("/var/cache/runwasi"));
    });
}

#[test]
fn the_ttl_and_the_size_bound_have_defaults() {
    with_env(&[(DIR, Some("/var/cache/runwasi"))], || {
        let config = CacheConfig::from_env().expect("the cache should be enabled");
        assert_eq!(config.ttl, DEFAULT_TTL);
        assert_eq!(config.max_bytes, DEFAULT_MAX_BYTES);
    });
}

#[test]
fn the_ttl_and_the_size_bound_come_from_the_environment() {
    for (ttl, max_bytes) in [
        ("90", "4096"),
        ("0", "0"),
        ("604800", "18446744073709551615"),
    ] {
        with_env(
            &[
                (DIR, Some("/var/cache/runwasi")),
                (TTL, Some(ttl)),
                (MAX_BYTES, Some(max_bytes)),
            ],
            || {
                let config = CacheConfig::from_env().expect("the cache should be enabled");
                assert_eq!(config.ttl, Duration::from_secs(ttl.parse().unwrap()));
                assert_eq!(config.max_bytes, max_bytes.parse::<u64>().unwrap());
            },
        );
    }
}

#[test]
fn a_ttl_that_does_not_parse_falls_back_to_the_default() {
    for ttl in UNPARSABLE {
        with_env(
            &[(DIR, Some("/var/cache/runwasi")), (TTL, Some(ttl))],
            || {
                let config = CacheConfig::from_env().expect("the cache should be enabled");
                assert_eq!(config.ttl, DEFAULT_TTL, "{ttl:?} should not have parsed");
            },
        );
    }
}

#[test]
fn a_size_bound_that_does_not_parse_falls_back_to_the_default() {
    for max_bytes in UNPARSABLE {
        with_env(
            &[
                (DIR, Some("/var/cache/runwasi")),
                (MAX_BYTES, Some(max_bytes)),
            ],
            || {
                let config = CacheConfig::from_env().expect("the cache should be enabled");
                assert_eq!(
                    config.max_bytes, DEFAULT_MAX_BYTES,
                    "{max_bytes:?} should not have parsed"
                );
            },
        );
    }
}

#[test]
fn any_directory_that_is_not_empty_enables_the_cache() {
    // nothing says the path has to look ordinary, only that it is there
    for dir in ["relative/cache", "cache dir/with a space", "."] {
        with_env(&[(DIR, Some(dir))], || {
            let config = CacheConfig::from_env().expect("the cache should be enabled");
            assert_eq!(
                config.dir,
                PathBuf::from(dir),
                "{dir:?} should be kept as it is"
            );
        });
    }
}

#[cfg(unix)]
#[test]
fn a_directory_the_system_accepts_enables_the_cache_whatever_its_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut raw = b"/tmp/runwasi-".to_vec();
    raw.extend_from_slice(&[0xff, 0xfe]);
    raw.extend_from_slice(b"-cache");
    let dir = OsString::from_vec(raw);
    temp_env::with_vars(
        [(DIR, Some(dir.clone())), (TTL, None), (MAX_BYTES, None)],
        || {
            let config = CacheConfig::from_env().expect("a non-empty directory enables the cache");
            assert_eq!(
                config.dir,
                PathBuf::from(&dir),
                "the directory is kept as the system gave it"
            );
        },
    );
}
