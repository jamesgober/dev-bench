//! Baseline storage for benchmark results.
//!
//! In `0.1.x`, baselines were passed in as inline `Option<Duration>`.
//! `0.4.x+` lifts that constraint: baselines can be persisted to and
//! loaded from disk via the [`BaselineStore`] trait. The default
//! backend is [`JsonFileBaselineStore`], which writes one JSON file
//! per `(scope, name)` key with atomic write-temp-rename semantics.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Persisted baseline for a single benchmark.
///
/// # Example
///
/// ```
/// use dev_bench::Baseline;
/// use std::time::Duration;
///
/// let b = Baseline {
///     name: "parse".into(),
///     mean_ns: Duration::from_nanos(1234).as_nanos() as u64,
///     samples: 1000,
///     ops_per_sec: 800_000.0,
/// };
/// assert_eq!(b.name, "parse");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// Stable name of the benchmark.
    pub name: String,
    /// Mean per-iteration duration, in nanoseconds.
    pub mean_ns: u64,
    /// Number of samples collected.
    pub samples: u64,
    /// Throughput at baseline, in ops/sec.
    pub ops_per_sec: f64,
}

impl Baseline {
    /// Convenience: extract `mean_ns` as a `Duration`.
    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.mean_ns)
    }
}

/// A storage backend for benchmark baselines.
///
/// Implementations MUST treat `load` as tolerant of missing data
/// (return `Ok(None)`). Implementations SHOULD treat `save` as
/// atomic — partial writes that survive a crash are unacceptable
/// because they would corrupt comparisons on the next run.
///
/// `scope` is a free-form key the caller uses to namespace baselines
/// (e.g. a git SHA, a branch name, or `"latest"`). The implementation
/// MUST treat `(scope, name)` as the identity of a baseline.
pub trait BaselineStore {
    /// Load a baseline if one exists for `(scope, name)`.
    fn load(&self, scope: &str, name: &str) -> io::Result<Option<Baseline>>;

    /// Persist a baseline atomically.
    fn save(&self, scope: &str, baseline: &Baseline) -> io::Result<()>;
}

/// Filesystem-backed JSON baseline store.
///
/// Keys baselines as `<root>/<scope>/<name>.json`. Save writes a
/// uniquely named temp file in the target directory, flushes it to
/// disk, then renames it over the target, so readers see either the
/// old file or the new one and concurrent saves do not trip over a
/// shared temp file.
///
/// # File names
///
/// `scope` and `name` are mapped to path components by replacing every
/// character outside `[A-Za-z0-9_.-]` with `_`. When that changes the
/// string, or the result would be unsafe or ambiguous as a path component
/// (empty, only dots, ending in a dot, a Windows device name such as `CON`
/// or `nul.txt`, or longer than 100 bytes), a 16-hex-digit hash of the
/// original string is appended (`parse_json-<hash>`). Two different
/// names therefore never share a file, and `..` can never escape `root`.
/// Plain names such as `parse_query` keep the simple `parse_query.json`.
///
/// Files written by older versions under the un-hashed name are still
/// read by [`load`](BaselineStore::load) when no hashed file exists, as
/// long as the `name` stored inside the file matches the requested name.
/// A file whose stored `name` does not match is treated as absent.
///
/// # Example
///
/// ```
/// use dev_bench::{Baseline, BaselineStore, JsonFileBaselineStore};
/// let dir = tempfile::tempdir().unwrap();
/// let store = JsonFileBaselineStore::new(dir.path());
/// let b = Baseline {
///     name: "parse".into(),
///     mean_ns: 1234,
///     samples: 100,
///     ops_per_sec: 800_000.0,
/// };
/// store.save("main", &b).unwrap();
/// let back = store.load("main", "parse").unwrap().unwrap();
/// assert_eq!(back, b);
/// ```
pub struct JsonFileBaselineStore {
    root: PathBuf,
}

impl JsonFileBaselineStore {
    /// Build a store rooted at `root`. The directory is created on
    /// first save if it does not exist.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, scope: &str, name: &str) -> PathBuf {
        self.root
            .join(path_component(scope))
            .join(format!("{}.json", path_component(name)))
    }

    /// The path used before collision-safe file names were introduced.
    /// `None` when the scope component is `..`, which would resolve
    /// outside `root`, so a legacy lookup never reads outside the store.
    fn legacy_path_for(&self, scope: &str, name: &str) -> Option<PathBuf> {
        let scope = sanitize(scope);
        if scope == ".." {
            return None;
        }
        let name = sanitize(name);
        Some(self.root.join(scope).join(format!("{name}.json")))
    }
}

/// Read and parse one baseline file. `Ok(None)` when the file does not
/// exist or belongs to a different benchmark name.
fn read_baseline(path: &Path, name: &str) -> io::Result<Option<Baseline>> {
    match fs::read(path) {
        Ok(bytes) => {
            let b: Baseline = serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid baseline at {}: {}", path.display(), e),
                )
            })?;
            if b.name == name {
                Ok(Some(b))
            } else {
                Ok(None)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

impl BaselineStore for JsonFileBaselineStore {
    fn load(&self, scope: &str, name: &str) -> io::Result<Option<Baseline>> {
        let path = self.path_for(scope, name);
        if let Some(b) = read_baseline(&path, name)? {
            return Ok(Some(b));
        }
        match self.legacy_path_for(scope, name) {
            Some(legacy) if legacy != path => read_baseline(&legacy, name),
            _ => Ok(None),
        }
    }

    fn save(&self, scope: &str, baseline: &Baseline) -> io::Result<()> {
        // serde_json writes NaN and infinity as `null`, which would make
        // the saved file unreadable on the next `load`.
        if !baseline.ops_per_sec.is_finite() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "baseline `{}` has a non-finite ops_per_sec ({})",
                    baseline.name, baseline.ops_per_sec
                ),
            ));
        }
        let path = self.path_for(scope, &baseline.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(baseline)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {}", e)))?;
        atomic_write(&path, &bytes)
    }
}

/// Atomic write: write to a temp sibling, flush it, then rename over
/// the target.
///
/// On the same filesystem, `rename` is atomic. This guarantees that a
/// reader either sees the complete previous file or the complete new
/// file, never a torn write. The temp name includes the process id and
/// a per-process counter so concurrent saves of the same key (threads
/// or processes) never write into each other's temp file. The temp file
/// is removed if any step fails.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_replacing(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// `fs::rename`, retried briefly on Windows when another save of the same
/// key is replacing the target at the same moment. Older Rust standard
/// libraries (and some file systems) rename with `MoveFileExW`, which
/// reports `PermissionDenied` or `NotFound` while a concurrent replace
/// holds the target. Other platforms and other errors return at once.
fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    const MAX_RETRIES: u64 = 20;
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && attempt < MAX_RETRIES
                    && matches!(
                        e.kind(),
                        io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                    )
                    && from.exists() =>
            {
                attempt += 1;
                std::thread::sleep(Duration::from_millis((attempt * 5).min(50)));
            }
            other => return other,
        }
    }
}

/// Longest sanitized component kept before a hash is appended.
const MAX_COMPONENT_LEN: usize = 100;

/// Map a scope or name to a single safe path component. See the
/// "File names" section on [`JsonFileBaselineStore`].
fn path_component(s: &str) -> String {
    let safe = sanitize(s);
    let needs_hash = safe != s
        || safe.is_empty()
        || safe.bytes().all(|b| b == b'.')
        || safe.ends_with('.')
        || safe.len() > MAX_COMPONENT_LEN
        || is_windows_device_name(&safe);
    if !needs_hash {
        return safe;
    }
    // `safe` is ASCII, so byte slicing is on a char boundary.
    let prefix = &safe[..safe.len().min(MAX_COMPONENT_LEN)];
    format!("{prefix}-{:016x}", fnv1a64(s.as_bytes()))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Windows reserves these device names in every directory, with or
/// without an extension (`nul`, `NUL.json`, `com1.txt`). Checked on every
/// platform so a baseline directory written on Linux can be checked out
/// on Windows.
fn is_windows_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let b = upper.as_bytes();
            b.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && b[3].is_ascii_digit()
        }
    }
}

/// 64-bit FNV-1a. Stable across platforms, Rust versions and runs,
/// unlike `std`'s `DefaultHasher`, which matters because the hash is part
/// of a file name on disk.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_baseline_through_json_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let b = Baseline {
            name: "parse_query".into(),
            mean_ns: 1234,
            samples: 1000,
            ops_per_sec: 810_000.0,
        };
        store.save("abc1234", &b).unwrap();
        let back = store.load("abc1234", "parse_query").unwrap().unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn missing_baseline_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let r = store.load("anything", "absent").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path().join("not_yet_existing"));
        let b = Baseline {
            name: "x".into(),
            mean_ns: 1,
            samples: 1,
            ops_per_sec: 1.0,
        };
        store.save("main", &b).unwrap();
        let back = store.load("main", "x").unwrap().unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let b1 = Baseline {
            name: "x".into(),
            mean_ns: 100,
            samples: 1,
            ops_per_sec: 10.0,
        };
        let b2 = Baseline {
            name: "x".into(),
            mean_ns: 200,
            samples: 2,
            ops_per_sec: 5.0,
        };
        store.save("main", &b1).unwrap();
        store.save("main", &b2).unwrap();
        let back = store.load("main", "x").unwrap().unwrap();
        assert_eq!(back, b2);
    }

    #[test]
    fn sanitize_blocks_path_traversal_in_scope_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let b = Baseline {
            name: "../escaped".into(),
            mean_ns: 1,
            samples: 1,
            ops_per_sec: 1.0,
        };
        // sanitize replaces / and . . combinations with safe chars,
        // so save lands inside the root regardless of input.
        store.save("../danger", &b).unwrap();
        // Root was not escaped: a sibling of `dir.path()` should not
        // contain anything new.
        let parent = dir.path().parent().unwrap();
        let entries_in_parent: usize = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path() != dir.path() && e.file_name().to_string_lossy().starts_with("danger")
            })
            .count();
        assert_eq!(entries_in_parent, 0);
    }

    #[test]
    fn corrupt_baseline_yields_invalid_data_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let path = store.path_for("main", "broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{ this is not json").unwrap();
        let err = store.load("main", "broken").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    fn bl(name: &str, mean_ns: u64) -> Baseline {
        Baseline {
            name: name.into(),
            mean_ns,
            samples: 1,
            ops_per_sec: 1.0,
        }
    }

    #[test]
    fn plain_names_keep_their_simple_file_name() {
        assert_eq!(path_component("parse_query"), "parse_query");
        assert_eq!(path_component("v1.2-beta"), "v1.2-beta");
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        store.save("main", &bl("parse_query", 1)).unwrap();
        assert!(dir.path().join("main").join("parse_query.json").is_file());
    }

    #[test]
    fn hash_is_stable_fnv1a() {
        // Published FNV-1a 64 test vectors. The hash is part of on-disk
        // file names, so it must never change.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn names_that_sanitize_alike_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        // All of these used to map to the same file.
        let names = ["a/b", "a_b", "a:b", "解析", "排序", "a b"];
        for (i, n) in names.iter().enumerate() {
            store.save("main", &bl(n, i as u64 + 1)).unwrap();
        }
        for (i, n) in names.iter().enumerate() {
            let back = store.load("main", n).unwrap().unwrap();
            assert_eq!(back.name, *n);
            assert_eq!(back.mean_ns, i as u64 + 1);
        }
        // Scopes are kept apart the same way.
        store.save("feature/x", &bl("k", 10)).unwrap();
        store.save("feature_x", &bl("k", 20)).unwrap();
        assert_eq!(store.load("feature/x", "k").unwrap().unwrap().mean_ns, 10);
        assert_eq!(store.load("feature_x", "k").unwrap().unwrap().mean_ns, 20);
    }

    #[test]
    fn dot_scopes_stay_inside_root() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("root");
        let store = JsonFileBaselineStore::new(&root);
        for scope in ["..", ".", "", "...", "x."] {
            store.save(scope, &bl("n", 7)).unwrap();
            assert_eq!(store.load(scope, "n").unwrap().unwrap().mean_ns, 7);
        }
        // Nothing was written next to `root`.
        let siblings: Vec<_> = fs::read_dir(outer.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings, vec![std::ffi::OsString::from("root")]);
        // Each odd scope got its own directory.
        assert_eq!(fs::read_dir(&root).unwrap().count(), 5);
    }

    #[test]
    fn windows_device_names_are_hashed() {
        for n in ["con", "NUL", "nul.txt", "Com1", "lpt9", "aux.json", "prn"] {
            assert!(is_windows_device_name(n), "{n}");
            assert_ne!(path_component(n), n);
        }
        for n in ["console", "com", "com10", "lpt", "nullable", "auxiliary"] {
            assert!(!is_windows_device_name(n), "{n}");
        }
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        store.save("aux", &bl("con", 3)).unwrap();
        assert_eq!(store.load("aux", "con").unwrap().unwrap().mean_ns, 3);
    }

    #[test]
    fn very_long_names_are_shortened() {
        let long = "x".repeat(400);
        let comp = path_component(&long);
        assert_eq!(comp.len(), MAX_COMPONENT_LEN + 17);
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        store.save("main", &bl(&long, 9)).unwrap();
        assert_eq!(store.load("main", &long).unwrap().unwrap().mean_ns, 9);
    }

    #[test]
    fn legacy_unhashed_file_is_still_read_for_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        // Simulate a file written by an older version for "a/b".
        let legacy = dir.path().join("main").join("a_b.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, serde_json::to_vec(&bl("a/b", 42)).unwrap()).unwrap();

        assert_eq!(store.load("main", "a/b").unwrap().unwrap().mean_ns, 42);
        // "a_b" maps to the same file name but the stored name differs,
        // so it is not mistaken for "a_b"'s baseline.
        assert!(store.load("main", "a_b").unwrap().is_none());

        // A new save for "a/b" goes to the hashed file and wins.
        store.save("main", &bl("a/b", 43)).unwrap();
        assert_eq!(store.load("main", "a/b").unwrap().unwrap().mean_ns, 43);
    }

    #[test]
    fn non_finite_ops_per_sec_is_rejected_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        for v in [f64::NAN, f64::INFINITY] {
            let mut b = bl("x", 1);
            b.ops_per_sec = v;
            let err = store.save("main", &b).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
        assert!(store.load("main", "x").unwrap().is_none());
    }

    #[test]
    fn save_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        store.save("main", &bl("x", 1)).unwrap();
        store.save("main", &bl("x", 2)).unwrap();
        let names: Vec<String> = fs::read_dir(dir.path().join("main"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["x.json".to_string()]);
    }

    #[test]
    fn concurrent_saves_of_one_key_all_succeed() {
        // With a fixed temp name, one thread's rename could move another
        // thread's half-written temp file, or fail with NotFound.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let handles: Vec<_> = (0..4u64)
            .map(|t| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let store = JsonFileBaselineStore::new(root);
                    for i in 0..10u64 {
                        store.save("main", &bl("hot", t * 100 + i + 1)).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let store = JsonFileBaselineStore::new(&root);
        let back = store.load("main", "hot").unwrap().unwrap();
        assert!(back.mean_ns > 0);
        let leftovers = fs::read_dir(root.join("main")).unwrap().count();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn empty_file_is_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileBaselineStore::new(dir.path());
        let path = store.path_for("main", "empty");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        let err = store.load("main", "empty").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("invalid baseline at"));
    }

    #[test]
    fn baseline_mean_returns_duration() {
        let b = Baseline {
            name: "x".into(),
            mean_ns: 5_000,
            samples: 1,
            ops_per_sec: 1.0,
        };
        assert_eq!(b.mean(), Duration::from_nanos(5_000));
    }
}
