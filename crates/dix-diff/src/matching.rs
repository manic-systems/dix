use std::{
  cmp::min,
  mem::swap,
};

use itertools::EitherOrBoth;

use crate::{
  VersionAmount,
  version::{
    VersionComponent,
    VersionPiece,
  },
};

/// Computes the Levenshtein distance between two slices.
fn levenshtein<T: Eq>(from: &[T], to: &[T]) -> usize {
  // Equal prefixes and suffixes never contribute to edit distance. Version
  // strings commonly share both, so trim them before allocating the DP rows.
  let prefix_len = from
    .iter()
    .zip(to)
    .take_while(|(left, right)| left == right)
    .count();
  let from = &from[prefix_len..];
  let to = &to[prefix_len..];

  let suffix_len = from
    .iter()
    .rev()
    .zip(to.iter().rev())
    .take_while(|(left, right)| left == right)
    .count();
  let from = &from[..from.len() - suffix_len];
  let to = &to[..to.len() - suffix_len];

  if from.is_empty() {
    return to.len();
  }
  if to.is_empty() {
    return from.len();
  }

  // Columns determine the DP buffer size, so use the shorter slice there.
  let (rows, columns) = if from.len() >= to.len() {
    (from, to)
  } else {
    (to, from)
  };

  let mut previous: Vec<usize> = (0..=columns.len()).collect();
  let mut current = vec![0; columns.len() + 1];

  for (row_index, row) in rows.iter().enumerate() {
    current[0] = row_index + 1;
    for (column_index, column) in columns.iter().enumerate() {
      let substitution_cost = usize::from(row != column);
      current[column_index + 1] = min(
        min(current[column_index] + 1, previous[column_index + 1] + 1),
        previous[column_index] + substitution_cost,
      );
    }
    swap(&mut previous, &mut current);
  }

  previous[columns.len()]
}

/// Finds a minimum-cost, order-preserving matching between two sequences.
///
/// The result contains `min(left_len, right_len)` `(left, right)` index pairs.
/// Every index occurs at most once, and both indexes increase monotonically.
fn minimum_cost_ordered_matching(
  left_len: usize,
  right_len: usize,
  cost: impl Fn(usize, usize) -> usize,
) -> Vec<(usize, usize)> {
  if left_len == 0 || right_len == 0 {
    return Vec::new();
  }

  let was_transposed = left_len > right_len;
  let (shorter_len, longer_len) = if was_transposed {
    (right_len, left_len)
  } else {
    (left_len, right_len)
  };

  // `best[i][j]` is the minimum cost of matching the first `i` items from the
  // shorter sequence into the first `j` items from the longer one.
  let mut best = vec![vec![usize::MAX; longer_len + 1]; shorter_len + 1];
  best[0].fill(0);

  for shorter in 1..=shorter_len {
    // Fewer than `shorter` columns cannot hold a matching of this size.
    for longer in shorter..=longer_len {
      let skip = best[shorter][longer - 1];
      let pair_cost = if was_transposed {
        cost(longer - 1, shorter - 1)
      } else {
        cost(shorter - 1, longer - 1)
      };
      let pair = best[shorter - 1][longer - 1].saturating_add(pair_cost);
      best[shorter][longer] = min(skip, pair);
    }
  }

  // Walk the table backwards. When skipping and pairing have equal cost,
  // prefer skipping the later item so matches stay as early as possible.
  let mut shorter = shorter_len;
  let mut longer = longer_len;
  let mut matching = Vec::with_capacity(shorter_len);
  while shorter > 0 {
    if longer > shorter && best[shorter][longer] == best[shorter][longer - 1] {
      longer -= 1;
      continue;
    }

    let pair = if was_transposed {
      (longer - 1, shorter - 1)
    } else {
      (shorter - 1, longer - 1)
    };
    matching.push(pair);
    shorter -= 1;
    longer -= 1;
  }

  matching.reverse();
  matching
}

/// Takes two ordered lists of versions and matches them while preserving their
/// order. The matching:
///
/// 1. Pairs every version from the shorter list.
/// 2. Minimizes the total edit distance between paired versions.
/// 3. Never crosses two version pairs.
///
/// Returns a vector of paired or unpaired versions (as `EitherOrBoth` enum).
#[must_use]
pub fn match_version_amounts<'a>(
  from: &'a [VersionAmount],
  to: &'a [VersionAmount],
) -> Vec<EitherOrBoth<&'a VersionAmount>> {
  // Early return for empty inputs
  if from.is_empty() {
    return to.iter().map(EitherOrBoth::Right).collect();
  }
  if to.is_empty() {
    return from.iter().map(EitherOrBoth::Left).collect();
  }

  // Equal-length order-preserving matchings have exactly one solution.
  if from.len() == to.len() {
    return from
      .iter()
      .zip(to)
      .map(|(from, to)| EitherOrBoth::Both(from, to))
      .collect();
  }

  // Pre-extract version components to avoid repetitive extraction
  let from_components: Vec<Vec<VersionComponent>> = from
    .iter()
    .map(|item| {
      item
        .version
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let to_components: Vec<Vec<VersionComponent>> = to
    .iter()
    .map(|item| {
      item
        .version
        .into_iter()
        .filter_map(VersionPiece::component)
        .collect()
    })
    .collect();

  let matchings = minimum_cost_ordered_matching(
    from_components.len(),
    to_components.len(),
    |from_index, to_index| {
      levenshtein(&from_components[from_index], &to_components[to_index])
    },
  );

  // Process matched pairs and retain the indexes left unmatched on either
  // side. Only the longer side can have any, but tracking both keeps this code
  // independent of whether the assignment matrix was transposed.
  let mut matched_from = vec![false; from.len()];
  let mut matched_to = vec![false; to.len()];
  let mut pairings =
    Vec::<EitherOrBoth<&VersionAmount>>::with_capacity(from.len() + to.len());

  for (from_index, to_index) in matchings {
    pairings.push(EitherOrBoth::Both(&from[from_index], &to[to_index]));
    matched_from[from_index] = true;
    matched_to[to_index] = true;
  }

  pairings.extend(
    from
      .iter()
      .enumerate()
      .filter(|(index, _)| !matched_from[*index])
      .map(|(_, version)| EitherOrBoth::Left(version)),
  );
  pairings.extend(
    to.iter()
      .enumerate()
      .filter(|(index, _)| !matched_to[*index])
      .map(|(_, version)| EitherOrBoth::Right(version)),
  );

  pairings
}

#[cfg(test)]
mod tests {
  use std::num::NonZeroUsize;

  use proptest::proptest;

  use super::*;
  use crate::{
    Version,
    version::VersionComponent,
  };

  fn version_amount(version: &str) -> VersionAmount {
    VersionAmount::new(version, NonZeroUsize::MIN)
  }

  fn matching_cost(costs: &[Vec<usize>], matching: &[(usize, usize)]) -> usize {
    matching
      .iter()
      .map(|&(row, column)| costs[row][column])
      .sum()
  }

  fn ordered_matching(costs: &[Vec<usize>]) -> Vec<(usize, usize)> {
    minimum_cost_ordered_matching(
      costs.len(),
      costs.first().map_or(0, Vec::len),
      |row, column| costs[row][column],
    )
  }

  fn brute_force_minimum_cost(
    costs: &[Vec<usize>],
    row: usize,
    next_column: usize,
    current_cost: usize,
    minimum_cost: &mut usize,
  ) {
    if row == costs.len() {
      *minimum_cost = min(*minimum_cost, current_cost);
      return;
    }

    for column in next_column..costs[row].len() {
      brute_force_minimum_cost(
        costs,
        row + 1,
        column + 1,
        current_cost + costs[row][column],
        minimum_cost,
      );
    }
  }

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
  fn ordered_matching_preserves_order_when_crossing_is_cheaper() {
    let costs = vec![vec![4, 1, 3], vec![2, 0, 5], vec![3, 2, 2]];

    assert_eq!(ordered_matching(&costs), vec![(0, 0), (1, 1), (2, 2)]);
  }

  #[test]
  fn ordered_matching_supports_wide_matrices() {
    let costs = vec![vec![10, 1, 9], vec![8, 7, 2]];

    assert_eq!(ordered_matching(&costs), vec![(0, 1), (1, 2)]);
    assert!(ordered_matching(&[]).is_empty());
  }

  #[test]
  fn ordered_matching_transposes_tall_matrices() {
    let costs = vec![vec![10, 8], vec![1, 7], vec![9, 2]];

    assert_eq!(ordered_matching(&costs), vec![(1, 0), (2, 1)]);
  }

  #[test]
  fn ordered_matching_matches_exhaustive_search() {
    // Exhaustively cover every 2x3 matrix whose costs are 0, 1, or 2.
    for encoded_costs in 0..3_usize.pow(6) {
      let mut encoded_costs = encoded_costs;
      let mut costs = vec![vec![0; 3]; 2];
      for row in &mut costs {
        for cost in row {
          *cost = encoded_costs % 3;
          encoded_costs /= 3;
        }
      }

      let matching = ordered_matching(&costs);
      let mut exhaustive_cost = usize::MAX;
      brute_force_minimum_cost(&costs, 0, 0, 0, &mut exhaustive_cost);

      assert_eq!(matching_cost(&costs, &matching), exhaustive_cost);

      let transposed = (0..3)
        .map(|column| costs.iter().map(|row| row[column]).collect::<Vec<_>>())
        .collect::<Vec<_>>();
      let transposed_matching = ordered_matching(&transposed);
      assert_eq!(
        matching_cost(&transposed, &transposed_matching),
        exhaustive_cost
      );
    }
  }

  #[test]
  fn match_version_amounts_matches_similar_versions() {
    let left = [version_amount("6.16.0"), version_amount("5.116.0")];
    let right = [version_amount("6.17.0"), version_amount("5.116.0-bin")];

    let matched = match_version_amounts(&left, &right);

    assert_eq!(matched.len(), 2);
    assert!(matched.iter().all(EitherOrBoth::has_left));
    assert!(matched.iter().all(EitherOrBoth::has_right));
  }

  #[test]
  fn match_version_amounts_empty() {
    let empty: &[VersionAmount] = &[];
    let versions = [version_amount("1.0.0")];

    let result = match_version_amounts(empty, &versions);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], EitherOrBoth::Right(_)));

    let result = match_version_amounts(&versions, empty);
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], EitherOrBoth::Left(_)));

    let result = match_version_amounts(empty, empty);
    assert!(result.is_empty());
  }

  #[test]
  fn match_version_amounts_exact_matches() {
    let a = [version_amount("1.0.0"), version_amount("2.0.0")];
    let b = [version_amount("1.0.0"), version_amount("2.0.0")];

    let result = match_version_amounts(&a, &b);
    let both_count = result
      .iter()
      .filter(|result| matches!(result, EitherOrBoth::Both(_, _)))
      .count();

    assert_eq!(both_count, 2);
  }

  #[test]
  fn match_version_amounts_unequal_sizes() {
    let a = [
      version_amount("1.0.0"),
      version_amount("2.0.0"),
      version_amount("3.0.0"),
    ];
    let b = [version_amount("1.0.0")];
    assert_eq!(match_version_amounts(&a, &b).len(), 3);

    let a = [version_amount("1.0.0")];
    let b = [
      version_amount("1.0.0"),
      version_amount("2.0.0"),
      version_amount("3.0.0"),
    ];
    assert_eq!(match_version_amounts(&a, &b).len(), 3);
  }

  #[test]
  fn match_version_amounts_prefers_exact_matches() {
    let a = [version_amount("1.0.0"), version_amount("2.0.0")];
    let b = [version_amount("1.0.1"), version_amount("2.0.0")];

    let result = match_version_amounts(&a, &b);

    assert!(result.iter().any(|result| {
      matches!(
        result,
        EitherOrBoth::Both(left, right)
          if left.version.name == "2.0.0" && right.version.name == "2.0.0"
      )
    }));
  }
}
