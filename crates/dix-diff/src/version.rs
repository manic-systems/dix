use std::{
  cmp,
  fmt,
};

use derive_more::{
  Deref,
  Display,
};

/// Separators used to split version strings.
const SEPARATORS: &[char] = &['.', '-', '_', '+', '*', '=', '×', ' '];

/// A version string with semantic comparison support.
///
/// Ordering compares separator-delimited components. Empty versions sort before
/// non-empty versions; after a shared prefix, text suffixes sort below the base
/// version and numeric suffixes sort above it. `Ord` uses the raw version
/// string as a final tie-breaker so distinct strings remain distinct
/// ordered-map keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
  pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionChangeOrdering {
  Ordered(cmp::Ordering),
  Unordered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparedVersionComponents {
  Ordered(cmp::Ordering),
  Equal,
  Unordered,
}

impl Version {
  #[must_use]
  pub fn new(version: impl Into<String>) -> Self {
    Self {
      name: version.into(),
    }
  }

  /// Iterate over components only.
  pub fn components(&self) -> impl Iterator<Item = VersionComponent<'_>> {
    Pieces::new(&self.name).filter_map(VersionPiece::component)
  }

  /// Iterate over all pieces (components and separators).
  #[must_use]
  pub fn iter(&self) -> Pieces<'_> {
    Pieces::new(&self.name)
  }

  pub(crate) fn change_ordering(&self, other: &Self) -> VersionChangeOrdering {
    match self.compare_components_with(other, |self_comp, other_comp| {
      if self_comp.is_git_hash_pair(other_comp) {
        ComparedVersionComponents::Unordered
      } else {
        ComparedVersionComponents::Ordered(self_comp.cmp(&other_comp))
      }
    }) {
      ComparedVersionComponents::Ordered(ordering) => {
        VersionChangeOrdering::Ordered(ordering)
      },
      ComparedVersionComponents::Equal => {
        VersionChangeOrdering::Ordered(cmp::Ordering::Equal)
      },
      ComparedVersionComponents::Unordered => VersionChangeOrdering::Unordered,
    }
  }

  fn compare_components_with(
    &self,
    other: &Self,
    mut compare_component: impl FnMut(
      VersionComponent<'_>,
      VersionComponent<'_>,
    ) -> ComparedVersionComponents,
  ) -> ComparedVersionComponents {
    let self_comps: Vec<_> = self.components().collect();
    let other_comps: Vec<_> = other.components().collect();
    let mut saw_unordered = false;

    for index in 0..self_comps.len().max(other_comps.len()) {
      let self_comp = self_comps.get(index).copied();
      let other_comp = other_comps.get(index).copied();

      if self_comp == other_comp {
        continue;
      }

      match compare_component_at(
        index,
        self_comp,
        other_comp,
        &mut compare_component,
      ) {
        ComparedVersionComponents::Equal
        | ComparedVersionComponents::Ordered(cmp::Ordering::Equal) => {},
        ComparedVersionComponents::Ordered(ordering) => {
          return ComparedVersionComponents::Ordered(ordering);
        },
        ComparedVersionComponents::Unordered => {
          saw_unordered = true;
        },
      }
    }

    if saw_unordered {
      ComparedVersionComponents::Unordered
    } else {
      ComparedVersionComponents::Equal
    }
  }
}

fn compare_component_at(
  index: usize,
  self_comp: Option<VersionComponent<'_>>,
  other_comp: Option<VersionComponent<'_>>,
  compare_component: &mut impl FnMut(
    VersionComponent<'_>,
    VersionComponent<'_>,
  ) -> ComparedVersionComponents,
) -> ComparedVersionComponents {
  let rank_ordering =
    component_rank(index, self_comp).cmp(&component_rank(index, other_comp));

  let (Some(self_comp), Some(other_comp)) = (self_comp, other_comp) else {
    return ComparedVersionComponents::Ordered(rank_ordering);
  };

  match compare_component(self_comp, other_comp) {
    ComparedVersionComponents::Unordered => {
      ComparedVersionComponents::Unordered
    },
    ComparedVersionComponents::Equal
    | ComparedVersionComponents::Ordered(cmp::Ordering::Equal) => {
      ComparedVersionComponents::Equal
    },
    ComparedVersionComponents::Ordered(ordering) => {
      ComparedVersionComponents::Ordered(rank_ordering.then(ordering))
    },
  }
}

fn component_rank(
  index: usize,
  component: Option<VersionComponent<'_>>,
) -> ComponentRank {
  // The first slot sorts empty versions below real versions. After a shared
  // prefix, text suffixes sort below the base and numeric suffixes above it.
  match (index, component.map(|component| component.is_numeric())) {
    (_, None) => ComponentRank::Missing,
    (0, Some(true)) => ComponentRank::FirstNumeric,
    (0, Some(false)) => ComponentRank::FirstText,
    (_, Some(false)) => ComponentRank::SuffixText,
    (_, Some(true)) => ComponentRank::SuffixNumeric,
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ComponentRank {
  SuffixText,
  Missing,
  FirstNumeric,
  FirstText,
  SuffixNumeric,
}

impl<T: Into<String>> From<T> for Version {
  fn from(s: T) -> Self {
    Self::new(s)
  }
}

impl PartialOrd for Version {
  fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for Version {
  fn cmp(&self, other: &Self) -> cmp::Ordering {
    let component_ordering =
      match self.compare_components_with(other, |self_comp, other_comp| {
        ComparedVersionComponents::Ordered(self_comp.cmp(&other_comp))
      }) {
        ComparedVersionComponents::Ordered(ordering) => ordering,
        ComparedVersionComponents::Equal
        | ComparedVersionComponents::Unordered => cmp::Ordering::Equal,
      };

    component_ordering.then_with(|| self.name.cmp(&other.name))
  }
}

impl<'a> IntoIterator for &'a Version {
  type Item = VersionPiece<'a>;
  type IntoIter = Pieces<'a>;

  fn into_iter(self) -> Self::IntoIter {
    Pieces::new(&self.name)
  }
}

/// Iterator over version pieces (components and separators).
#[derive(Clone, Copy)]
pub struct Pieces<'a> {
  remaining: &'a str,
}

impl<'a> Pieces<'a> {
  const fn new(s: &'a str) -> Self {
    Self { remaining: s }
  }
}

#[expect(clippy::copy_iterator)]
impl<'a> Iterator for Pieces<'a> {
  type Item = VersionPiece<'a>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.remaining.is_empty() {
      return None;
    }

    let first = self.remaining.chars().next()?;

    if SEPARATORS.contains(&first) {
      let len = first.len_utf8();
      let sep = &self.remaining[..len];
      self.remaining = &self.remaining[len..];
      return Some(VersionPiece::Separator(sep));
    }

    let len = self
      .remaining
      .chars()
      .take_while(|c| !SEPARATORS.contains(c))
      .map(char::len_utf8)
      .sum();

    let comp = &self.remaining[..len];
    self.remaining = &self.remaining[len..];
    Some(VersionPiece::Component(VersionComponent(comp)))
  }
}

/// Either a component or separator from a version string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionPiece<'a> {
  Component(VersionComponent<'a>),
  Separator(&'a str),
}

impl<'a> VersionPiece<'a> {
  #[must_use]
  pub const fn component(self) -> Option<VersionComponent<'a>> {
    match self {
      VersionPiece::Component(c) => Some(c),
      VersionPiece::Separator(_) => None,
    }
  }

  #[must_use]
  pub const fn separator(self) -> Option<&'a str> {
    match self {
      VersionPiece::Component(_) => None,
      VersionPiece::Separator(s) => Some(s),
    }
  }
}

/// A single version component (numeric or text).
#[derive(Display, Debug, Clone, Copy, Deref, PartialEq, Eq)]
pub struct VersionComponent<'a>(&'a str);

impl VersionComponent<'_> {
  #[must_use]
  pub fn is_numeric(&self) -> bool {
    !self.0.is_empty() && self.0.bytes().all(|b| b.is_ascii_digit())
  }

  #[must_use]
  pub fn as_u64(&self) -> Option<u64> {
    self.is_numeric().then(|| self.0.parse().ok()).flatten()
  }

  fn is_git_hash_pair(&self, other: Self) -> bool {
    is_hex_hash_component(self.0)
      && is_hex_hash_component(other.0)
      && (has_ascii_hex_letter(self.0) || has_ascii_hex_letter(other.0))
  }
}

fn is_hex_hash_component(component: &str) -> bool {
  (7..=40).contains(&component.len())
    && component.bytes().all(|b| b.is_ascii_hexdigit())
}

fn has_ascii_hex_letter(component: &str) -> bool {
  component
    .bytes()
    .any(|b| matches!(b, b'a'..=b'f' | b'A'..=b'F'))
}

impl PartialOrd for VersionComponent<'_> {
  fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for VersionComponent<'_> {
  fn cmp(&self, other: &Self) -> cmp::Ordering {
    match (self.is_numeric(), other.is_numeric()) {
      (true, true) => {
        match (self.as_u64(), other.as_u64()) {
          (Some(a), Some(b)) => a.cmp(&b),
          _ => self.0.cmp(other.0),
        }
      },
      (false, false) => {
        match (self.0, other.0) {
          ("pre", _) => cmp::Ordering::Less,
          (_, "pre") => cmp::Ordering::Greater,
          _ => self.0.cmp(other.0),
        }
      },
      (true, false) => cmp::Ordering::Less,
      (false, true) => cmp::Ordering::Greater,
    }
  }
}

impl fmt::Display for Version {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.name)
  }
}

impl fmt::Write for Version {
  fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
    fmt::write(&mut self.name, args)
  }

  fn write_str(&mut self, s: &str) -> fmt::Result {
    self.name.push_str(s);
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use proptest::proptest;

  use super::{
    Version,
    VersionChangeOrdering,
    VersionComponent,
    VersionPiece,
  };

  // tests to ensure that [`Version::cmp`] is a total order
  proptest! {
    #[test]
    fn test_version_transitivity(
      a in ".*",
      b in ".*",
      c in ".*",
    ) {
      let a = Version::new(a);
      let b = Version::new(b);
      let c = Version::new(c);

      // a < b < c -> a < c
      assert!(!(a < b && b < c) || a < c);
      // a < c < b -> a < b
      assert!(!(a < c && c < b) || a < b);
      // b < a < c -> b < c
      assert!(!(b < a && a < c) || b < c);
      // b < c < a -> b < a
      assert!(!(b < c && c < a) || b < a);
      // c < a < b -> c < b
      assert!(!(c < a && a < b) || c < b);
      // c < b < a -> c < a
      assert!(!(c < b && b < a) || c < a);
    }

    #[test]
    fn test_version_reflexivity(
      a in ".*",
    ) {
      let a = Version::new(a);
      let b = a.clone();

      assert_eq!(a.cmp(&a), std::cmp::Ordering::Equal);
      assert_eq!(a, b);
    }

    #[test]
    fn test_version_antisymmetry(
      a in ".*",
      b in ".*",
    ) {
      let a = Version::new(a);
      let b = Version::new(b);

      assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
      if a.cmp(&b) == std::cmp::Ordering::Equal {
        assert_eq!(a, b);
      }
    }

  }

  #[test]
  fn version_transitivity_regression_for_empty_text_and_numeric_components() {
    let empty = Version::new("");
    let numeric = Version::new("0");
    let text = Version::new(":");

    assert!(empty < text);
    assert!(numeric < text);
    assert!(empty < numeric);
  }

  #[test]
  fn version_suffix_order_is_transitive() {
    let base = Version::new("1");
    let text = Version::new("1-alpha");
    let hyphen_numeric = Version::new("1-0");
    let dot_numeric = Version::new("1.0");

    assert!(text < base);
    assert!(base < hyphen_numeric);
    assert!(base < dot_numeric);
    assert!(text < hyphen_numeric);
    assert!(text < dot_numeric);
  }

  #[test]
  fn version_order_keeps_component_equal_strings_distinct() {
    use std::collections::BTreeSet;

    let dotted = Version::new("1.0");
    let hyphenated = Version::new("1-0");
    let mut versions = BTreeSet::new();

    versions.insert(dotted.clone());
    versions.insert(hyphenated.clone());

    assert_ne!(dotted, hyphenated);
    assert_ne!(dotted.cmp(&hyphenated), std::cmp::Ordering::Equal);
    assert_eq!(versions.len(), 2);
  }

  #[test]
  fn change_ordering_treats_component_equal_strings_as_changed() {
    let dotted = Version::new("1.0");
    let hyphenated = Version::new("1-0");

    assert_eq!(
      dotted.change_ordering(&hyphenated),
      VersionChangeOrdering::Ordered(std::cmp::Ordering::Equal),
    );
    assert_ne!(dotted.cmp(&hyphenated), std::cmp::Ordering::Equal);
  }

  #[test]
  fn version_component_iter() {
    let version = "132.1.2test234-1-man----.--.......---------..---";

    assert_eq!(
      Version::new(version)
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect::<Vec<_>>(),
      [
        VersionComponent("132"),
        VersionComponent("1"),
        VersionComponent("2test234"),
        VersionComponent("1"),
        VersionComponent("man")
      ]
    );
  }

  #[test]
  fn version_comparison() {
    assert!(Version::new("2.0.0") > Version::new("1.9.9"));
    assert!(Version::new("2.1.0") > Version::new("2.0.9"));
    assert!(Version::new("2.0.1") > Version::new("2.0.0"));
    assert!(Version::new("1.0.0") > Version::new("1.0.0-pre"));
    assert!(Version::new("1.0.0") > Version::new("1.0.0-alpha"));
    assert!(Version::new("1.0.0-beta") > Version::new("1.0.0-alpha"));
    assert!(Version::new("1.0.0-beta.11") > Version::new("1.0.0-beta.2"));
    assert_eq!(Version::new("1.0.0"), Version::new("1.0.0"));
  }

  #[test]
  fn change_ordering_treats_git_short_hash_pair_as_unordered() {
    let old = Version::new("0.11.1-946aa34");
    let new = Version::new("0.11.1-3564204");

    assert_eq!(old.change_ordering(&new), VersionChangeOrdering::Unordered);
    assert_eq!(new.change_ordering(&old), VersionChangeOrdering::Unordered);
  }

  #[test]
  fn change_ordering_treats_git_long_hash_pair_as_unordered() {
    let old = Version::new("0bf8387987c21bf2f8ed41d2575a8f22b139687f");
    let new = Version::new("cd1931314beafeebc957964c65802961e283411e");

    assert_eq!(old.change_ordering(&new), VersionChangeOrdering::Unordered);
    assert_eq!(new.change_ordering(&old), VersionChangeOrdering::Unordered);
  }

  #[test]
  fn change_ordering_uses_non_hash_components_before_hashes() {
    let old = Version::new("25.05.31pre20250531_946aa34");
    let new = Version::new("25.05.31pre20250601_3564204");

    assert_eq!(
      old.change_ordering(&new),
      VersionChangeOrdering::Ordered(std::cmp::Ordering::Less),
    );
    assert_eq!(
      new.change_ordering(&old),
      VersionChangeOrdering::Ordered(std::cmp::Ordering::Greater),
    );
  }

  #[test]
  fn change_ordering_keeps_numeric_components_ordered() {
    let old = Version::new("1.0-1234567");
    let new = Version::new("1.0-2345678");

    assert_eq!(
      old.change_ordering(&new),
      VersionChangeOrdering::Ordered(std::cmp::Ordering::Less),
    );
  }

  #[test]
  fn version_piece_iterator_includes_separators() {
    let version = Version::new("1.2.3-alpha");
    let pieces: Vec<_> = version.into_iter().collect();
    assert_eq!(pieces.len(), 7);
    assert!(matches!(pieces[0], VersionPiece::Component(_)));
    assert!(matches!(pieces[1], VersionPiece::Separator(".")));
    assert!(matches!(pieces[2], VersionPiece::Component(_)));
    assert!(matches!(pieces[3], VersionPiece::Separator(".")));
    assert!(matches!(pieces[4], VersionPiece::Component(_)));
    assert!(matches!(pieces[5], VersionPiece::Separator("-")));
    assert!(matches!(pieces[6], VersionPiece::Component(_)));
  }

  #[test]
  fn version_piece_methods() {
    let comp = VersionPiece::Component(VersionComponent("123"));
    let sep = VersionPiece::Separator("-");

    assert_eq!(comp.component(), Some(VersionComponent("123")));
    assert_eq!(comp.separator(), None);
    assert_eq!(sep.component(), None);
    assert_eq!(sep.separator(), Some("-"));
  }

  #[test]
  fn version_component_is_numeric() {
    assert!(VersionComponent("123").is_numeric());
    assert!(VersionComponent("0").is_numeric());
    assert!(!VersionComponent("abc").is_numeric());
    assert!(!VersionComponent("123abc").is_numeric());
    assert!(!VersionComponent("").is_numeric());
    assert!(!VersionComponent("12.3").is_numeric());
  }

  #[test]
  fn version_component_as_u64() {
    assert_eq!(VersionComponent("123").as_u64(), Some(123));
    assert_eq!(VersionComponent("0").as_u64(), Some(0));
    assert_eq!(
      VersionComponent("18446744073709551615").as_u64(),
      Some(u64::MAX)
    );
    assert_eq!(VersionComponent("abc").as_u64(), None);
    assert_eq!(VersionComponent("123abc").as_u64(), None);
    assert_eq!(VersionComponent("").as_u64(), None);
  }

  #[test]
  fn component_comparison_numeric() {
    assert!(VersionComponent("10") > VersionComponent("2"));
    assert!(VersionComponent("2") < VersionComponent("10"));
    assert_eq!(VersionComponent("5"), VersionComponent("5"));
  }

  #[test]
  fn component_comparison_text() {
    assert!(VersionComponent("beta") > VersionComponent("alpha"));
    assert!(VersionComponent("rc") > VersionComponent("beta"));
    assert_eq!(VersionComponent("stable"), VersionComponent("stable"));
  }

  #[test]
  fn component_comparison_pre_special_case() {
    assert!(VersionComponent("pre") < VersionComponent("alpha"));
    assert!(VersionComponent("pre") > VersionComponent("1")); // text > numeric
    assert!(VersionComponent("alpha") > VersionComponent("pre"));
  }

  #[test]
  fn component_comparison_mixed_types() {
    assert!(VersionComponent("2") < VersionComponent("alpha"));
    assert!(VersionComponent("alpha") > VersionComponent("2"));
  }

  #[test]
  fn component_comparison_mixed_alphanumeric() {
    assert_eq!(VersionComponent("2test234"), VersionComponent("2test234"));
    assert!(VersionComponent("abc123") > VersionComponent("123abc"));
  }

  #[test]
  fn version_comparison_equal_lengths() {
    assert!(Version::new("1.2.3") > Version::new("1.2.2"));
    assert!(Version::new("1.2.3") < Version::new("1.2.4"));
    assert_eq!(Version::new("1.2.3"), Version::new("1.2.3"));
  }

  #[test]
  fn version_comparison_suffix_edge_cases() {
    assert!(Version::new("1.0.0") > Version::new("1.0.0-alpha"));
    assert!(Version::new("1.0.0-alpha") < Version::new("1.0.0"));
    assert!(Version::new("1.0.0-1") > Version::new("1.0.0"));
    assert!(Version::new("1.0.0-alpha") < Version::new("1.0.0-1"));
    assert!(Version::new("1.0.0-alpha") < Version::new("1.0.0-beta"));
    assert!(Version::new("1.0.0-1") < Version::new("1.0.0-2"));
    assert!(Version::new("1.0.0-9") < Version::new("1.0.0-10"));
  }

  #[test]
  fn version_comparison_numeric_extensions() {
    assert!(Version::new("1.0.0.1") > Version::new("1.0.0"));
    assert!(Version::new("1.0.0") < Version::new("1.0.0.1"));
  }

  #[test]
  fn version_display() {
    let v1 = Version::new("1.2.3");
    assert_eq!(format!("{v1}"), "1.2.3");
  }

  #[test]
  fn version_write() {
    use std::fmt::Write;

    let mut v = Version::new("1.0");
    write!(v, ".{}-beta", 2).unwrap();
    assert_eq!(v.name, "1.0.2-beta");
  }

  #[test]
  fn empty_version() {
    let v = Version::new("");
    assert_eq!(v.components().count(), 0);
  }

  #[test]
  fn version_with_only_separators() {
    let v = Version::new("...---___");
    assert_eq!(v.components().count(), 0);
  }

  #[test]
  fn version_from_string() {
    let v1: Version = "1.2.3".into();
    assert_eq!(v1.name, "1.2.3");

    let v2: Version = String::from("4.5.6").into();
    assert_eq!(v2.name, "4.5.6");
  }

  #[test]
  fn various_separators() {
    let v = Version::new("1_2+3=4*5×6 7");
    let comps: Vec<_> = v.components().collect();
    assert_eq!(comps.len(), 7); // 1, 2, 3, 4, 5, 6, 7
    assert_eq!(comps[0].0, "1");
    assert_eq!(comps[6].0, "7");
  }

  #[test]
  fn complex_version_parsing() {
    let v = Version::new("firefox-123.0.1_beta-1-x86_64");
    let comps: Vec<_> = v.components().collect();
    assert_eq!(comps[0].0, "firefox");
    assert_eq!(comps[1].0, "123");
    assert_eq!(comps[2].0, "0");
    assert_eq!(comps[3].0, "1"); // _ is a separator, so "1_beta" splits
    assert_eq!(comps[4].0, "beta");
    assert_eq!(comps[5].0, "1");
    assert_eq!(comps[6].0, "x86"); // _ is a separator, so "x86_64" splits
    assert_eq!(comps[7].0, "64");
  }

  #[test]
  fn version_clone_and_eq() {
    let v1 = Version::new("1.0.0");
    let v2 = v1.clone();
    assert_eq!(v1, v2);
    assert_eq!(v1.name, v2.name);
  }

  #[test]
  fn version_hash_consistency() {
    use std::collections::HashSet;

    let v1 = Version::new("1.0.0");
    let v2 = Version::new("1.0.0");
    let v3 = Version::new("2.0.0");

    let mut set = HashSet::new();
    set.insert(v1);
    assert!(set.contains(&v2));
    assert!(!set.contains(&v3));
  }
}
