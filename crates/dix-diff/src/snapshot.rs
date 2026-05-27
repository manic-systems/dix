use rustc_hash::FxHashSet;
use size::Size;

use crate::Version;

#[derive(Debug, Eq, PartialEq)]
pub struct Package {
  pub name:    String,
  pub version: Version,
}

impl Package {
  #[must_use]
  pub fn new(name: impl Into<String>, version: impl Into<Version>) -> Self {
    Self {
      name:    name.into(),
      version: version.into(),
    }
  }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct PackageSnapshot {
  pub packages:       Vec<Package>,
  pub selected_names: FxHashSet<String>,
  pub closure_size:   Size,
}

impl PackageSnapshot {
  #[must_use]
  pub fn new(
    packages: impl Into<Vec<Package>>,
    selected_names: impl IntoIterator<Item = String>,
    closure_size: Size,
  ) -> Self {
    Self {
      packages: packages.into(),
      selected_names: selected_names.into_iter().collect(),
      closure_size,
    }
  }
}
