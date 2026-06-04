// Test utilities afixture_path) nd infrastructure for database testing.
//
// This module provides utilities to create temporary SQLite databases
// with the Nix store schema for testing purposes.

use std::{
  fs,
  path::PathBuf,
};

use eyre::Result;
use rusqlite::Connection;
use tempfile::TempDir;

/// Test database builder for creating temporary `SQLite` databases
/// with the Nix store schema.
pub struct TestDbBuilder {
  temp_dir: TempDir,
  db_path:  PathBuf,
}

impl TestDbBuilder {
  /// Creates a new test database builder with the Nix store schema initialized.
  pub fn new() -> Result<Self> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let builder = Self { temp_dir, db_path };
    builder.init_schema()?;

    Ok(builder)
  }

  /// Returns the path to the database file.
  pub fn db_path(&self) -> &std::path::Path {
    &self.db_path
  }

  /// Returns the actual filesystem path for a given fixture path.
  ///
  /// Converts a path like `/nix/store/xxx-name` to a path under
  /// `tempdir/nix/store/xxx-name` so that canonicalized paths will
  /// contain `/nix/store/` in them.
  pub fn resolve_fixture_path(&self, fixture_path: &str) -> PathBuf {
    // All paths are created under tempdir/nix/store/
    // so they canonicalize to paths containing /nix/store/
    self
      .temp_dir
      .path()
      .join(fixture_path.strip_prefix("/").unwrap_or(fixture_path))
  }

  /// Opens a read-write connection to the database.
  fn open(&self) -> Result<Connection> {
    Ok(rusqlite::Connection::open(&self.db_path)?)
  }

  /// Initializes the Nix store database schema.
  fn init_schema(&self) -> Result<()> {
    let conn = self.open()?;
    conn.execute_batch(
      "CREATE TABLE IF NOT EXISTS ValidPaths (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        path TEXT NOT NULL UNIQUE,
        hash TEXT NOT NULL,
        registrationTime INTEGER NOT NULL,
        deriver TEXT,
        narSize INTEGER NOT NULL,
        ultimate INTEGER,
        sigs TEXT,
        ca TEXT
      );

      CREATE TABLE IF NOT EXISTS Refs (
        referrer INTEGER NOT NULL,
        reference INTEGER NOT NULL,
        PRIMARY KEY (referrer, reference),
        FOREIGN KEY (referrer) REFERENCES ValidPaths(id),
        FOREIGN KEY (reference) REFERENCES ValidPaths(id)
      );

      CREATE INDEX IF NOT EXISTS IndexRefs ON Refs(referrer);
      CREATE INDEX IF NOT EXISTS IndexPath ON ValidPaths(path);",
    )?;
    Ok(conn.close().map_err(|(_, err)| err)?)
  }

  /// Retrieves the id using a path
  pub fn get_id(&self, path: &str) -> Option<i64> {
    let fs_path = self.resolve_fixture_path(path);
    let canonical_path = fs_path.canonicalize().ok()?;
    let path_str = canonical_path.to_string_lossy();
    let conn = self.open().ok()?;
    let mut stmt = conn
      .prepare("SELECT id FROM ValidPaths WHERE path = ?1 LIMIT 1")
      .ok()?;

    let id = stmt
      .query_row([&path_str], |row| row.get::<_, i64>(0))
      .ok()?;
    drop(stmt);

    let _ = conn.close().map_err(|(_, err)| err);
    Some(id)
  }

  /// Adds a valid path to the database, creating the filesystem directory.
  ///
  /// Creates the directory under `tempdir/nix/store/` so that canonicalized
  /// paths contain `/nix/store/`.
  pub fn add_valid_path(&self, path: &str, nar_size: i64) -> Result<i64> {
    let fs_path = self.resolve_fixture_path(path);
    fs::create_dir_all(&fs_path)?;
    let canonical_path = fs_path.canonicalize()?;
    let path_str = canonical_path.to_string_lossy();

    let conn = self.open()?;
    let mut stmt = conn.prepare(
      "INSERT INTO ValidPaths (path, hash, registrationTime, narSize) VALUES \
       (?1, ?2, ?3, ?4) RETURNING id",
    )?;

    let id = stmt.query_row(
      [&path_str, "test-hash", "1234567890", &nar_size.to_string()],
      |row| row.get::<_, i64>(0),
    )?;
    drop(stmt);

    conn.close().map_err(|(_, err)| err)?;
    Ok(id)
  }

  /// Adds a reference relationship between two valid paths.
  pub fn add_reference(
    &self,
    referrer_id: i64,
    reference_id: i64,
  ) -> Result<()> {
    let conn = self.open()?;
    conn.execute(
      "INSERT INTO Refs (referrer, reference) VALUES (?1, ?2)",
      [referrer_id, reference_id],
    )?;
    Ok(conn.close().map_err(|(_, err)| err)?)
  }

  /// Creates a complete closure with paths and references.
  pub fn create_closure(
    &self,
    paths: Vec<(&str, i64)>,
    refs: Vec<(&str, &str)>,
  ) -> Result<()> {
    let mut path_ids = std::collections::HashMap::new();

    for (path, nar_size) in paths {
      let id = self.add_valid_path(path, nar_size)?;
      path_ids.insert(path.to_string(), id);
    }

    for (referrer, reference) in refs {
      let referrer_id = path_ids
        .get(referrer)
        .copied()
        .or_else(|| self.get_id(referrer))
        .ok_or_else(|| eyre::eyre!("Referrer not found: {referrer}"))?;
      let reference_id = path_ids
        .get(reference)
        .copied()
        .or_else(|| self.get_id(reference))
        .ok_or_else(|| eyre::eyre!("Reference not found: {reference}"))?;
      self.add_reference(referrer_id, reference_id)?;
    }

    Ok(())
  }
}

/// Standard test fixtures for Nix store paths.
pub mod fixtures {
  /// Returns a standard store path prefix.
  pub fn store_prefix() -> &'static str {
    "/nix/store/00000000000000000000000000000000-"
  }

  /// Creates a full store path from a package name.
  pub fn store_path(name: &str) -> String {
    format!("{}{}", store_prefix(), name)
  }

  /// Creates a system derivation path.
  pub fn system_path(name: &str) -> String {
    format!("{}-system", store_path(name))
  }
}

/// Creates a test database with a simple closure (root + 2 deps).
pub fn create_simple_test_db() -> Result<TestDbBuilder> {
  let db = TestDbBuilder::new()?;

  let root = fixtures::store_path("root-package");
  let dep1 = fixtures::store_path("dependency-1.0");
  let dep2 = fixtures::store_path("dependency-2.0");

  db.create_closure(vec![(&root, 100), (&dep1, 50), (&dep2, 50)], vec![
    (&root, &dep1),
    (&root, &dep2),
  ])?;

  Ok(db)
}

/// Creates a test database simulating a NixOS system closure.
pub fn create_system_test_db() -> Result<TestDbBuilder> {
  let db = TestDbBuilder::new()?;

  // System-level paths
  let system_old = fixtures::system_path("nixos-25.11");
  let system_path_old =
    format!("{}-system-path", fixtures::store_path("nixos-25.11"));
  let system_new = fixtures::system_path("nixos-25.12");
  let system_path_new =
    format!("{}-system-path", fixtures::store_path("nixos-25.12"));

  // Package paths
  let glibc_old = fixtures::store_path("glibc-2.38");
  let glibc_new = fixtures::store_path("glibc-2.39");
  let bash = fixtures::store_path("bash-5.2.15");
  let coreutils = fixtures::store_path("coreutils-9.3");

  // Create all paths with references:
  // system -> system-path -> packages
  db.create_closure(
    vec![
      (&system_old, 0),
      (&system_path_old, 1000),
      (&glibc_old, 50_000_000),
      (&bash, 5_000_000),
      (&coreutils, 10_000_000),
    ],
    vec![
      (&system_old, &system_path_old),
      (&system_path_old, &bash),
      (&system_path_old, &coreutils),
      (&bash, &glibc_old),
      (&coreutils, &glibc_old),
    ],
  )?;
  db.create_closure(
    vec![
      (&system_new, 0),
      (&system_path_new, 1000),
      (&glibc_new, 50_000_000),
    ],
    vec![
      (&system_new, &system_path_new),
      (&system_path_new, &bash),
      (&system_path_new, &coreutils),
      (&bash, &glibc_new),
      (&coreutils, &glibc_new),
    ],
  )?;

  Ok(db)
}

/// Creates a test database with a diamond dependency pattern (A->B,C->D).
pub fn create_diamond_test_db() -> Result<TestDbBuilder> {
  let db = TestDbBuilder::new()?;

  let a = fixtures::store_path("package-a");
  let b = fixtures::store_path("package-b");
  let c = fixtures::store_path("package-c");
  let d = fixtures::store_path("package-d");

  db.create_closure(vec![(&a, 1000), (&b, 500), (&c, 500), (&d, 250)], vec![
    (&a, &b),
    (&a, &c),
    (&b, &d),
    (&c, &d),
  ])?;

  Ok(db)
}

/// Edge case test fixtures.
pub mod edge_cases {
  use super::*;

  /// Creates a test database with a self-referencing path.
  pub fn create_self_reference_test_db() -> Result<TestDbBuilder> {
    let db = TestDbBuilder::new()?;
    let path = fixtures::store_path("self-referential");
    let id = db.add_valid_path(&path, 500)?;
    db.add_reference(id, id)?;
    Ok(db)
  }

  /// Creates a test database with a circular dependency (A->B->C->A).
  pub fn create_circular_test_db() -> Result<TestDbBuilder> {
    let db = TestDbBuilder::new()?;

    let a = fixtures::store_path("circular-a");
    let b = fixtures::store_path("circular-b");
    let c = fixtures::store_path("circular-c");

    let id_a = db.add_valid_path(&a, 100)?;
    let id_b = db.add_valid_path(&b, 100)?;
    let id_c = db.add_valid_path(&c, 100)?;

    db.add_reference(id_a, id_b)?;
    db.add_reference(id_b, id_c)?;
    db.add_reference(id_c, id_a)?;

    Ok(db)
  }

  /// Creates a test database with a wide dependency tree.
  pub fn create_wide_tree_test_db(n_children: usize) -> Result<TestDbBuilder> {
    let db = TestDbBuilder::new()?;

    let root = fixtures::store_path("wide-root");
    let root_id = db.add_valid_path(&root, 1000)?;

    for i in 0..n_children {
      let child = fixtures::store_path(&format!("wide-child-{i}"));
      let child_id = db.add_valid_path(&child, 50)?;
      db.add_reference(root_id, child_id)?;
    }

    Ok(db)
  }

  /// Creates a test database with a deeply nested chain.
  pub fn create_deep_chain_test_db(depth: usize) -> Result<TestDbBuilder> {
    let db = TestDbBuilder::new()?;

    if depth == 0 {
      return Ok(db);
    }

    let mut prev_id =
      db.add_valid_path(&fixtures::store_path("deep-0"), 100)?;

    for i in 1..depth {
      let path = fixtures::store_path(&format!("deep-{i}"));
      let id = db.add_valid_path(&path, 100)?;
      db.add_reference(prev_id, id)?;
      prev_id = id;
    }

    Ok(db)
  }
}

#[cfg(test)]
mod tests {
  use size::Size;

  use super::*;
  use crate::store::{
    DbConnection,
    StoreBackend,
  };

  #[test]
  fn test_db_builder_creation() {
    let db = TestDbBuilder::new().unwrap();
    assert!(db.db_path().exists());
  }

  #[test]
  fn test_add_valid_path() {
    let db = TestDbBuilder::new().unwrap();
    let path = fixtures::store_path("test-package");
    let id = db.add_valid_path(&path, 1000).unwrap();
    assert!(id > 0);
  }

  #[test]
  fn test_create_closure() {
    let db = TestDbBuilder::new().unwrap();
    let root = fixtures::store_path("root");
    let dep = fixtures::store_path("dep");

    db.create_closure(vec![(&root, 100), (&dep, 50)], vec![(&root, &dep)])
      .unwrap();

    // Verify via database query
    let conn = db.open().unwrap();
    let count: i64 = conn
      .query_row("SELECT COUNT(*) FROM ValidPaths", [], |row| row.get(0))
      .unwrap();
    assert_eq!(count, 2);
  }

  #[test]
  fn test_db_query_closure_size() {
    let db = create_simple_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let root_fixture = fixtures::store_path("root-package");
    let root = db.resolve_fixture_path(&root_fixture);

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    assert!(conn.connected());
    let size = conn.query_closure_size(&root).unwrap();
    assert_eq!(size, Size::from_bytes(200)); // 100 + 50 + 50
    conn.close().unwrap();
    assert!(!conn.connected());
  }

  #[test]
  fn test_db_query_closure_path_info() {
    let db = create_simple_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let root_fixture = fixtures::store_path("root-package");
    let root = db.resolve_fixture_path(&root_fixture);

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let info = conn.query_closure_path_info(&root).unwrap();
    assert_eq!(info.len(), 3);
    assert_eq!(
      info.iter().map(|path| path.nar_size().bytes()).sum::<i64>(),
      200,
    );
    conn.close().unwrap();
  }

  #[test]
  fn test_db_query_dependents() {
    let db = create_diamond_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let a_fixture = fixtures::store_path("package-a");
    let a = db.resolve_fixture_path(&a_fixture);

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let dependents = conn.query_dependents(&a).unwrap();
    assert_eq!(dependents.len(), 4);
    conn.close().unwrap();
  }

  #[test]
  fn test_db_query_system_derivations() {
    let db = create_system_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let system_fixture = fixtures::system_path("nixos-25.11");
    let system = db.resolve_fixture_path(&system_fixture);

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let derivations = conn.query_system_derivations(&system).unwrap();
    assert!(!derivations.is_empty());
    conn.close().unwrap();
  }

  #[test]
  fn test_self_referential_path() {
    let db = edge_cases::create_self_reference_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let path_fixture = fixtures::store_path("self-referential");
    let path = db.resolve_fixture_path(&path_fixture);

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let size = conn.query_closure_size(&path).unwrap();
    assert_eq!(size, Size::from_bytes(500));
    conn.close().unwrap();
  }

  #[test]
  fn test_circular_dependencies() {
    let db = edge_cases::create_circular_test_db().unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    for letter in ["a", "b", "c"] {
      let path = db.resolve_fixture_path(&fixtures::store_path(&format!(
        "circular-{letter}"
      )));
      let size = conn.query_closure_size(&path).unwrap();
      assert_eq!(
        size,
        Size::from_bytes(300),
        "Wrong size for circular-{letter}"
      );
    }
    conn.close().unwrap();
  }

  #[test]
  fn test_wide_tree() {
    let db = edge_cases::create_wide_tree_test_db(100).unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let path = db.resolve_fixture_path(&fixtures::store_path("wide-root"));

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let size = conn.query_closure_size(&path).unwrap();
    assert_eq!(size, Size::from_bytes(6000)); // 1000 + 100*50

    let dependents = conn.query_dependents(&path).unwrap();
    assert_eq!(dependents.len(), 101); // root + 100 children
    conn.close().unwrap();
  }

  #[test]
  fn test_deep_chain() {
    let db = edge_cases::create_deep_chain_test_db(100).unwrap();
    let db_path = db.db_path().to_string_lossy().to_string();
    let path = db.resolve_fixture_path(&fixtures::store_path("deep-0"));

    let mut conn = DbConnection::new(&db_path);
    conn.connect().unwrap();
    let size = conn.query_closure_size(&path).unwrap();
    assert_eq!(size, Size::from_bytes(10000)); // 100 * 100

    let dependents = conn.query_dependents(&path).unwrap();
    assert_eq!(dependents.len(), 100);
    conn.close().unwrap();
  }
}
