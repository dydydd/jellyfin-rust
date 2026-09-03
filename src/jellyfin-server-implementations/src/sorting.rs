use std::cmp::Ordering;

use chrono::{DateTime, TimeZone, Utc};

/// The item field used for index-number ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexNumberOrderKey {
    pub index_number: Option<i32>,
}

impl IndexNumberOrderKey {
    /// Creates an index-number ordering key.
    #[must_use]
    pub const fn new(index_number: Option<i32>) -> Self {
        Self { index_number }
    }
}

/// Stateless index-number comparer.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexNumberComparer;

impl IndexNumberComparer {
    /// Compares two index-number keys, sorting missing numbers first.
    #[must_use]
    pub fn compare(x: &IndexNumberOrderKey, y: &IndexNumberOrderKey) -> Ordering {
        compare_index_number(x, y)
    }
}

/// Compares two index-number keys, sorting missing numbers first.
#[must_use]
pub fn compare_index_number(x: &IndexNumberOrderKey, y: &IndexNumberOrderKey) -> Ordering {
    x.index_number.cmp(&y.index_number)
}

/// The item field used for parent-index-number ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentIndexNumberOrderKey {
    pub parent_index_number: Option<i32>,
}

impl ParentIndexNumberOrderKey {
    /// Creates a parent-index-number ordering key.
    #[must_use]
    pub const fn new(parent_index_number: Option<i32>) -> Self {
        Self {
            parent_index_number,
        }
    }
}

/// Stateless parent-index-number comparer.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParentIndexNumberComparer;

impl ParentIndexNumberComparer {
    /// Compares two parent-index-number keys, sorting missing numbers first.
    #[must_use]
    pub fn compare(x: &ParentIndexNumberOrderKey, y: &ParentIndexNumberOrderKey) -> Ordering {
        compare_parent_index_number(x, y)
    }
}

/// Compares two parent-index-number keys, sorting missing numbers first.
#[must_use]
pub fn compare_parent_index_number(
    x: &ParentIndexNumberOrderKey,
    y: &ParentIndexNumberOrderKey,
) -> Ordering {
    x.parent_index_number.cmp(&y.parent_index_number)
}

/// The item fields used for premiere-date ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PremiereDateOrderKey {
    pub premiere_date: Option<DateTime<Utc>>,
    pub production_year: Option<i32>,
}

impl PremiereDateOrderKey {
    /// Creates a premiere-date ordering key.
    #[must_use]
    pub const fn new(premiere_date: Option<DateTime<Utc>>, production_year: Option<i32>) -> Self {
        Self {
            premiere_date,
            production_year,
        }
    }
}

/// Stateless premiere-date comparer.
#[derive(Debug, Clone, Copy, Default)]
pub struct PremiereDateComparer;

impl PremiereDateComparer {
    /// Compares two premiere-date keys using production year as a fallback.
    #[must_use]
    pub fn compare(x: &PremiereDateOrderKey, y: &PremiereDateOrderKey) -> Ordering {
        compare_premiere_date(x, y)
    }
}

/// Compares two premiere-date keys using production year as a fallback.
///
/// A valid production year represents January 1 of that year. Missing and
/// invalid years use the same minimum date as the official comparer.
#[must_use]
pub fn compare_premiere_date(x: &PremiereDateOrderKey, y: &PremiereDateOrderKey) -> Ordering {
    premiere_sort_date(x).cmp(&premiere_sort_date(y))
}

fn premiere_sort_date(item: &PremiereDateOrderKey) -> DateTime<Utc> {
    item.premiere_date.unwrap_or_else(|| {
        item.production_year
            .filter(|year| (1..=9999).contains(year))
            .and_then(|year| Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single())
            .unwrap_or_else(minimum_jellyfin_date)
    })
}

fn minimum_jellyfin_date() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .expect("year 1 must be representable by chrono")
}

/// The item fields used to reproduce Jellyfin's aired-episode ordering.
///
/// Query layers can project their item representation into this key without
/// coupling the comparer to a particular database or API model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiredEpisodeOrderKey {
    pub is_episode: bool,
    pub parent_index_number: Option<i32>,
    pub index_number: Option<i32>,
    pub airs_after_season_number: Option<i32>,
    pub airs_before_season_number: Option<i32>,
    pub airs_before_episode_number: Option<i32>,
    pub premiere_date: Option<DateTime<Utc>>,
}

impl AiredEpisodeOrderKey {
    /// Creates a key for an item that is not an episode.
    #[must_use]
    pub const fn other() -> Self {
        Self {
            is_episode: false,
            parent_index_number: None,
            index_number: None,
            airs_after_season_number: None,
            airs_before_season_number: None,
            airs_before_episode_number: None,
            premiere_date: None,
        }
    }

    /// Creates a key for an episode whose ordering metadata is not known yet.
    #[must_use]
    pub const fn episode() -> Self {
        Self {
            is_episode: true,
            parent_index_number: None,
            index_number: None,
            airs_after_season_number: None,
            airs_before_season_number: None,
            airs_before_episode_number: None,
            premiere_date: None,
        }
    }
}

/// Stateless aired-episode comparer suitable for in-memory query results.
#[derive(Debug, Clone, Copy, Default)]
pub struct AiredEpisodeOrderComparer;

impl AiredEpisodeOrderComparer {
    /// Compares two projected item keys using Jellyfin's aired-episode rules.
    #[must_use]
    pub fn compare(x: &AiredEpisodeOrderKey, y: &AiredEpisodeOrderKey) -> Ordering {
        compare_aired_episode_order(x, y)
    }
}

/// Compares projected item keys using Jellyfin's aired-episode rules.
///
/// Episodes sort before other item types. Regular episodes use season and
/// episode numbers, with premiere date as a tie-breaker. Specials use their
/// before/after placement metadata.
#[must_use]
pub fn compare_aired_episode_order(x: &AiredEpisodeOrderKey, y: &AiredEpisodeOrderKey) -> Ordering {
    match (x.is_episode, y.is_episode) {
        (false, false) => Ordering::Equal,
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (true, true) => compare_episodes_and_specials(x, y),
    }
}

fn compare_episodes_and_specials(x: &AiredEpisodeOrderKey, y: &AiredEpisodeOrderKey) -> Ordering {
    let x_is_special = x.parent_index_number.unwrap_or(-1) == 0;
    let y_is_special = y.parent_index_number.unwrap_or(-1) == 0;

    match (x_is_special, y_is_special) {
        (true, true) => special_compare_value(x).cmp(&special_compare_value(y)),
        (false, false) => compare_regular_episodes(x, y),
        (false, true) => compare_episode_to_special(x, y),
        (true, false) => compare_episode_to_special(y, x).reverse(),
    }
}

fn compare_episode_to_special(
    episode: &AiredEpisodeOrderKey,
    special: &AiredEpisodeOrderKey,
) -> Ordering {
    let episode_season = episode.parent_index_number.unwrap_or(-1);
    let special_season = special
        .airs_after_season_number
        .or(special.airs_before_season_number)
        .unwrap_or(-1);

    let season_order = episode_season.cmp(&special_season);
    if season_order != Ordering::Equal {
        return season_order;
    }

    if special.airs_after_season_number.is_some() {
        return Ordering::Less;
    }

    let Some(special_episode) = special.airs_before_episode_number else {
        return Ordering::Greater;
    };
    let Some(episode_number) = episode.index_number else {
        return Ordering::Equal;
    };

    if episode_number == special_episode {
        Ordering::Greater
    } else {
        episode_number.cmp(&special_episode)
    }
}

fn special_compare_value(item: &AiredEpisodeOrderKey) -> i64 {
    let season = item
        .airs_after_season_number
        .or(item.airs_before_season_number)
        .unwrap_or(0);
    let after_season = i64::from(item.airs_after_season_number.is_some());

    i64::from(season) * 1_000_000_000
        + after_season * 1_000_000
        + i64::from(item.airs_before_episode_number.unwrap_or(0)) * 1_000
        + i64::from(item.index_number.unwrap_or(0))
}

fn compare_regular_episodes(x: &AiredEpisodeOrderKey, y: &AiredEpisodeOrderKey) -> Ordering {
    let episode_value = |item: &AiredEpisodeOrderKey| {
        i64::from(item.parent_index_number.unwrap_or(-1)) * 1_000
            + i64::from(item.index_number.unwrap_or(-1))
    };

    let number_order = episode_value(x).cmp(&episode_value(y));
    if number_order == Ordering::Equal
        && let (Some(x_date), Some(y_date)) = (&x.premiere_date, &y.premiere_date)
    {
        return x_date.cmp(y_date);
    }

    number_order
}
