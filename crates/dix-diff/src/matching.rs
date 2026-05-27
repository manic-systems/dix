use std::{
  cmp::min,
  collections::HashSet,
  mem::swap,
};

use itertools::EitherOrBoth;
use pathfinding::{
  kuhn_munkres,
  matrix::Matrix,
};

use crate::{
  CountedVersion,
  Version,
  version::{
    VersionComponent,
    VersionPiece,
  },
};

pub(crate) trait VersionSource {
  fn version(&self) -> &Version;
}

impl VersionSource for Version {
  fn version(&self) -> &Version {
    self
  }
}

impl VersionSource for CountedVersion {
  fn version(&self) -> &Version {
    &self.version
  }
}

/// Computes the Levenshtein distance between two slices.
pub(crate) fn levenshtein<T: Eq>(from: &[T], to: &[T]) -> usize {
  let (from_len, to_len) = (from.len(), to.len());

  if from_len == 0 {
    return to_len;
  }
  if to_len == 0 {
    return from_len;
  }

  // Use 'from' as the shorter slice for memory efficiency
  let (from, to, from_len, to_len) = if from_len > to_len {
    (to, from, to_len, from_len)
  } else {
    (from, to, from_len, to_len)
  };

  let mut prev: Vec<usize> = (0..=to_len).collect();
  let mut curr = vec![0; to_len + 1];

  for i in 1..=from_len {
    curr[0] = i;
    for j in 1..=to_len {
      let cost = usize::from(from[i - 1] != to[j - 1]);
      curr[j] = min(min(curr[j - 1] + 1, prev[j] + 1), prev[j - 1] + cost);
    }
    swap(&mut prev, &mut curr);
  }

  prev[to_len]
}

fn version_edit_distance<T: VersionSource>(from: &T, to: &T) -> usize {
  let from_components: Vec<_> = from
    .version()
    .into_iter()
    .filter_map(VersionPiece::component)
    .collect();
  let to_components: Vec<_> = to
    .version()
    .into_iter()
    .filter_map(VersionPiece::component)
    .collect();

  levenshtein(&from_components, &to_components)
}

fn closest_version_index<T: VersionSource>(
  source: &T,
  candidates: &[T],
) -> usize {
  let mut best_index = 0;
  let mut best_distance = version_edit_distance(source, &candidates[0]);

  for (index, candidate) in candidates.iter().enumerate().skip(1) {
    let distance = version_edit_distance(source, candidate);
    if distance < best_distance {
      best_index = index;
      best_distance = distance;
    }
  }

  best_index
}

fn version_assignments<T>(from: &[T], to: &[T]) -> Vec<(usize, usize)>
where
  T: VersionSource + PartialEq,
{
  if from == to {
    return (0..from.len()).map(|index| (index, index)).collect();
  }

  if from.len() == 1 {
    return vec![(0, closest_version_index(&from[0], to))];
  }

  if from.len() == 2 && to.len() == 2 {
    return two_by_two_assignments(from, to);
  }

  hungarian_assignments(from, to)
}

fn two_by_two_assignments<T: VersionSource>(
  from: &[T],
  to: &[T],
) -> Vec<(usize, usize)> {
  let direct_cost = version_edit_distance(&from[0], &to[0])
    .saturating_add(version_edit_distance(&from[1], &to[1]));
  let crossed_cost = version_edit_distance(&from[0], &to[1])
    .saturating_add(version_edit_distance(&from[1], &to[0]));

  if direct_cost <= crossed_cost {
    vec![(0, 0), (1, 1)]
  } else {
    vec![(0, 1), (1, 0)]
  }
}

fn hungarian_assignments<T: VersionSource>(
  from: &[T],
  to: &[T],
) -> Vec<(usize, usize)> {
  let from_components: Vec<Vec<VersionComponent>> = from
    .iter()
    .map(|version| {
      version
        .version()
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let to_components: Vec<Vec<VersionComponent>> = to
    .iter()
    .map(|version| {
      version
        .version()
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let mut distances = Matrix::new(from.len(), to.len(), 0_i32);

  for i in 0..from.len() {
    for j in 0..to.len() {
      distances[(i, j)] =
        i32::try_from(levenshtein(&from_components[i], &to_components[j]))
          .unwrap_or_else(|err| {
            tracing::warn!("Distance must fit in i32: {err}");
            i32::MAX
          });
    }
  }

  let (_cost, matchings) =
    kuhn_munkres::kuhn_munkres_min::<i32, Matrix<i32>>(&distances);

  matchings.into_iter().enumerate().collect()
}

fn materialize_pairings<'a, T: Ord>(
  from: &'a [T],
  to: &'a [T],
  assignments: Vec<(usize, usize)>,
  swapped: bool,
) -> Vec<EitherOrBoth<&'a T>> {
  let mut remaining = (0..to.len()).collect::<HashSet<usize>>();
  let mut pairings =
    Vec::<EitherOrBoth<&T>>::with_capacity(from.len() + to.len());

  for (i, j) in assignments {
    pairings.push(EitherOrBoth::Both(&from[i], &to[j]));
    remaining.remove(&j);
  }

  if !remaining.is_empty() {
    let mut remaining = remaining.iter().map(|&j| &to[j]).collect::<Vec<_>>();
    remaining.sort_unstable();
    pairings.extend(remaining.into_iter().map(EitherOrBoth::Right));
  }

  if swapped {
    pairings.into_iter().map(EitherOrBoth::flip).collect()
  } else {
    pairings
  }
}

/// Takes two lists of versions and tries to match them using the Hungarian
/// algorithm. The matching attempts to minimize the edit distance between
/// version pairs, which means:
///
/// 1. Versions with minimal edit distance are paired
/// 2. The natural ordering of versions is preserved where possible
///
/// Returns a vector of paired or unpaired versions (as `EitherOrBoth` enum).
pub(crate) fn match_version_lists<'a, T>(
  mut from: &'a [T],
  mut to: &'a [T],
) -> Vec<EitherOrBoth<&'a T>>
where
  T: VersionSource + PartialEq + Ord,
{
  // Early return for empty inputs
  if from.is_empty() {
    return to.iter().map(EitherOrBoth::Right).collect();
  }
  if to.is_empty() {
    return from.iter().map(EitherOrBoth::Left).collect();
  }

  // Hungarian algorithm requires #rows <= #columns
  // Since the edit distance is symmetric, we can swap inputs if needed
  let swapped = if from.len() > to.len() {
    (to, from) = (from, to);
    true
  } else {
    false
  };

  let assignments = version_assignments(from, to);
  materialize_pairings(from, to, assignments, swapped)
}

pub fn match_counted_version_lists<'a>(
  from: &'a [CountedVersion],
  to: &'a [CountedVersion],
) -> Vec<EitherOrBoth<&'a CountedVersion>> {
  match_version_lists(from, to)
}

#[cfg(test)]
mod tests {
  use proptest::proptest;

  use super::*;
  use crate::{
    Version,
    version::VersionComponent,
  };

  proptest! {
    #[test]
    fn no_crash_edit_dist(from in r"(\PC-)*(\PC)?", to in r"(\PC-)*(\PC)?") {
      let from = Version::from(from);
      let from: Vec<VersionComponent> = from
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      let to = Version::from(to);
      let to: Vec<VersionComponent> = to
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      levenshtein(&from, &to);
    }

    #[test]
    fn symmetry_edit_dist(from in r"(\PC-)*(\PC)?", to in r"(\PC-)*(\PC)?") {
      let from = Version::from(from);
      let from: Vec<VersionComponent> = from
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      let to = Version::from(to);
      let to: Vec<VersionComponent> = to
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect();

      let forward = levenshtein(&from, &to);
      let backward = levenshtein(&to, &from);
      assert_eq!(forward, backward);
    }
  }

  #[test]
  fn basic_component_edit_dist() {
    let from = Version::from("foo-123.0-man-pages".to_owned());
    let from: Vec<VersionComponent> = from
      .into_iter()
      .filter_map(VersionPiece::component)
      .collect();

    let to = Version::from("foo-123.4.12-man-pages".to_owned());
    let to: Vec<VersionComponent> =
      to.into_iter().filter_map(VersionPiece::component).collect();

    assert_eq!(levenshtein(&from, &to), 2);
  }

  #[test]
  fn levenshtein_distance_tests() {
    assert_eq!(
      levenshtein(
        &"kitten".chars().collect::<Vec<_>>(),
        &"sitting".chars().collect::<Vec<_>>()
      ),
      3
    );
    assert_eq!(
      levenshtein(
        &"".chars().collect::<Vec<_>>(),
        &"hello".chars().collect::<Vec<_>>()
      ),
      5
    );
    assert_eq!(
      levenshtein(
        &"abcd".chars().collect::<Vec<_>>(),
        &"dcba".chars().collect::<Vec<_>>()
      ),
      4
    );
    assert_eq!(
      levenshtein(
        &"12345".chars().collect::<Vec<_>>(),
        &"12345".chars().collect::<Vec<_>>()
      ),
      0
    );
    assert_eq!(
      levenshtein(
        &"distance".chars().collect::<Vec<_>>(),
        &"difference".chars().collect::<Vec<_>>()
      ),
      5
    );
  }

  #[test]
  fn levenshtein_edge_cases() {
    assert_eq!(levenshtein::<char>(&[], &[]), 0);
    assert_eq!(levenshtein(&['a'], &[]), 1);
    assert_eq!(levenshtein(&[], &['a']), 1);
    assert_eq!(levenshtein(&['a'], &['b']), 1);
    assert_eq!(levenshtein(&['a'], &['a']), 0);
    assert_eq!(
      levenshtein(
        &"ab".chars().collect::<Vec<_>>(),
        &"ba".chars().collect::<Vec<_>>()
      ),
      2
    );
    assert_eq!(
      levenshtein(
        &"ABC".chars().collect::<Vec<_>>(),
        &"abc".chars().collect::<Vec<_>>()
      ),
      3
    );

    let long = "a".repeat(1000);
    assert_eq!(
      levenshtein(
        &long.chars().collect::<Vec<_>>(),
        &long.chars().collect::<Vec<_>>()
      ),
      0
    );

    let long_a = "a".repeat(1000);
    let long_b = "b".repeat(1000);
    assert_eq!(
      levenshtein(
        &long_a.chars().collect::<Vec<_>>(),
        &long_b.chars().collect::<Vec<_>>()
      ),
      1000
    );

    assert_eq!(
      levenshtein(
        &"こんにちは".chars().collect::<Vec<_>>(),
        &"こんばんは".chars().collect::<Vec<_>>()
      ),
      2
    );
    assert_eq!(
      levenshtein(
        &"abc".chars().collect::<Vec<_>>(),
        &"abcabc".chars().collect::<Vec<_>>()
      ),
      3
    );
    assert_eq!(levenshtein(&[1, 2, 3], &[1, 2, 3, 4, 5]), 2);
  }

  #[test]
  fn match_version_lists_matches_similar_versions() {
    let left = [Version::new("6.16.0"), Version::new("5.116.0")];
    let right = [Version::new("6.17.0"), Version::new("5.116.0-bin")];

    let matched = match_version_lists(&left, &right);

    assert_eq!(matched.len(), 2);
    assert!(matched.iter().all(EitherOrBoth::has_left));
    assert!(matched.iter().all(EitherOrBoth::has_right));
  }

  #[test]
  fn match_version_lists_pairs_single_versions() {
    let left = [Version::new("1.0.0")];
    let right = [Version::new("2.0.0")];

    let result = match_version_lists(&left, &right);

    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], EitherOrBoth::Both(_, _)));
  }

  #[test]
  fn match_version_lists_pairs_single_with_closest_candidate() {
    let left = [Version::new("1.0.0")];
    let right = [
      Version::new("3.5.7"),
      Version::new("1.0.1"),
      Version::new("8.8.8"),
    ];

    let result = match_version_lists(&left, &right);

    assert_eq!(result.len(), 3);
    assert!(matches!(
      result[0],
      EitherOrBoth::Both(_, right) if right.name == "1.0.1"
    ));
    assert_eq!(
      result
        .iter()
        .filter(|result| matches!(result, EitherOrBoth::Right(_)))
        .count(),
      2
    );
  }

  #[test]
  fn match_version_lists_uses_cheapest_two_by_two_pairing() {
    let left = [Version::new("1.0.0"), Version::new("10.0.0")];
    let right = [Version::new("10.0.1"), Version::new("1.0.1")];

    let result = match_version_lists(&left, &right);

    assert_eq!(result.len(), 2);
    assert!(matches!(
      result[0],
      EitherOrBoth::Both(left, right)
        if left.name == "1.0.0" && right.name == "1.0.1"
    ));
    assert!(matches!(
      result[1],
      EitherOrBoth::Both(left, right)
        if left.name == "10.0.0" && right.name == "10.0.1"
    ));
  }

  #[test]
  fn match_version_lists_empty() {
    let empty: &[Version] = &[];
    let versions = [Version::new("1.0.0")];

    let result = match_version_lists(empty, &versions);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], EitherOrBoth::Right(_)));

    let result = match_version_lists(&versions, empty);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], EitherOrBoth::Left(_)));

    let result = match_version_lists(empty, empty);
    assert!(result.is_empty());
  }

  #[test]
  fn match_version_lists_exact_matches() {
    let a = [Version::new("1.0.0"), Version::new("2.0.0")];
    let b = [Version::new("1.0.0"), Version::new("2.0.0")];

    let result = match_version_lists(&a, &b);
    let both_count = result
      .iter()
      .filter(|result| matches!(result, EitherOrBoth::Both(_, _)))
      .count();

    assert_eq!(both_count, 2);
  }

  #[test]
  fn match_version_lists_unequal_sizes() {
    let a = [
      Version::new("1.0.0"),
      Version::new("2.0.0"),
      Version::new("3.0.0"),
    ];
    let b = [Version::new("1.0.0")];
    assert_eq!(match_version_lists(&a, &b).len(), 3);

    let a = [Version::new("1.0.0")];
    let b = [
      Version::new("1.0.0"),
      Version::new("2.0.0"),
      Version::new("3.0.0"),
    ];
    assert_eq!(match_version_lists(&a, &b).len(), 3);
  }

  #[test]
  fn match_version_lists_prefers_exact_matches() {
    let a = [Version::new("1.0.0"), Version::new("2.0.0")];
    let b = [Version::new("1.0.1"), Version::new("2.0.0")];

    let result = match_version_lists(&a, &b);

    assert!(result.iter().any(|result| {
      matches!(
        result,
        EitherOrBoth::Both(left, right)
          if left.name == "2.0.0" && right.name == "2.0.0"
      )
    }));
  }
}
