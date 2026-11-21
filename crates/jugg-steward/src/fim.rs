//! File integrity monitoring for the release, config and unit files.
//! Baseline is re-taken after each verified update; any drift is an event.

use crate::{Event, Severity};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn snapshot(roots: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

fn walk(p: &Path, out: &mut BTreeMap<PathBuf, String>) {
    if let Ok(meta) = std::fs::symlink_metadata(p) {
        if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    walk(&e.path(), out);
                }
            }
        } else if meta.is_file() {
            if let Ok(bytes) = std::fs::read(p) {
                out.insert(p.to_path_buf(), jugg_crypto::sha256_hex(&bytes));
            }
        }
    }
}

pub fn diff(baseline: &BTreeMap<PathBuf, String>, now: &BTreeMap<PathBuf, String>) -> Vec<Event> {
    let mut ev = Vec::new();
    for (path, hash) in now {
        match baseline.get(path) {
            None => ev.push(Event::new("fim", "file_added", Severity::High, 0.7, path.display().to_string(), json!({}))),
            Some(h) if h != hash => ev.push(Event::new("fim", "file_modified", Severity::Critical, 0.85, path.display().to_string(), json!({}))),
            _ => {}
        }
    }
    for path in baseline.keys().filter(|p| !now.contains_key(*p)) {
        ev.push(Event::new("fim", "file_removed", Severity::High, 0.7, path.display().to_string(), json!({})));
    }
    ev
}
