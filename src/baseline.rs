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
/// Keys baselines as `<root>/<scope>/<name>.json`. Save uses
/// write-temp-rename to be atomic on the same filesystem.
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
        let safe_scope = sanitize(scope);
        let safe_name = sanitize(name);
        self.root.join(safe_scope).join(format!("{safe_name}.json"))
    }
}

impl BaselineStore for JsonFileBaselineStore {
    fn load(&self, scope: &str, name: &str) -> io::Result<Option<Baseline>> {
        let path = self.path_for(scope, name);
        match fs::read(&path) {
            Ok(bytes) => {
                let b: Baseline = serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid baseline at {}: {}", path.display(), e),
                    )
                })?;
                Ok(Some(b))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn save(&self, scope: &str, baseline: &Baseline) -> io::Result<()> {
        let path = self.path_for(scope, &baseline.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(baseline)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {}", e)))?;
        atomic_write(&path, &bytes)
    }
}

/// Atomic write: write to a temp sibling, then rename over the target.
///
/// On the same filesystem, `rename` is atomic. This guarantees that a
/// reader either sees the complete previous file or the complete new
/// file, never a torn write.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let temp = parent.join(format!(".{}.tmp", file_name.to_string_lossy()));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)?;
    Ok(())
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
