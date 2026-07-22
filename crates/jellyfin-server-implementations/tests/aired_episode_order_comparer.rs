use std::cmp::Ordering;

use chrono::{TimeZone, Utc};
use jellyfin_server_implementations::{AiredEpisodeOrderComparer, AiredEpisodeOrderKey};

#[test]
fn aired_episode_order_matches_the_official_matrix() {
    for (x, y, expected) in official_cases() {
        assert_eq!(AiredEpisodeOrderComparer::compare(&x, &y), expected);
        assert_eq!(
            AiredEpisodeOrderComparer::compare(&y, &x),
            expected.reverse()
        );
    }
}

fn official_cases() -> Vec<(AiredEpisodeOrderKey, AiredEpisodeOrderKey, Ordering)> {
    vec![
        (other(), other(), Ordering::Equal),
        (other(), episode(), Ordering::Greater),
        (episode(), episode(), Ordering::Equal),
        (
            numbered_episode(1, 1),
            numbered_episode(1, 1),
            Ordering::Equal,
        ),
        (
            numbered_episode(1, 2),
            numbered_episode(1, 1),
            Ordering::Greater,
        ),
        (
            numbered_episode(2, 1),
            numbered_episode(1, 1),
            Ordering::Greater,
        ),
        (special(1), special(1), Ordering::Equal),
        (special(2), special(1), Ordering::Greater),
        (numbered_episode(1, 1), special(1), Ordering::Greater),
        (numbered_episode(1, 1), special(2), Ordering::Greater),
        (numbered_episode(1, 2), special(1), Ordering::Greater),
        (numbered_episode(1, 2), special(1), Ordering::Greater),
        (numbered_episode(1, 1), special(2), Ordering::Greater),
        (
            special_after_season(1, 1),
            numbered_episode(1, 1),
            Ordering::Greater,
        ),
        (
            numbered_episode(3, 1),
            special_after_season(1, 1),
            Ordering::Greater,
        ),
        (
            numbered_episode(3, 1),
            special_after_season_before_episode(1, 1, 2),
            Ordering::Greater,
        ),
        (
            numbered_episode(1, 1),
            special_before_season(1, 1),
            Ordering::Greater,
        ),
        (
            numbered_episode(1, 2),
            special_before_episode(1, 1, 2),
            Ordering::Greater,
        ),
        (
            episode_in_season_without_number(1),
            special_before_episode(1, 1, 2),
            Ordering::Equal,
        ),
        (
            numbered_episode(1, 3),
            special_before_episode(1, 1, 2),
            Ordering::Greater,
        ),
        (
            premiered_episode(2021, 9, 12),
            premiered_episode(2021, 9, 12),
            Ordering::Equal,
        ),
        (
            premiered_episode(2021, 9, 11),
            premiered_episode(2021, 9, 12),
            Ordering::Less,
        ),
        (
            premiered_episode(2021, 9, 12),
            premiered_episode(2021, 9, 11),
            Ordering::Greater,
        ),
    ]
}

const fn other() -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey::other()
}

const fn episode() -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey::episode()
}

const fn numbered_episode(season: i32, episode_number: i32) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        parent_index_number: Some(season),
        index_number: Some(episode_number),
        ..episode()
    }
}

const fn episode_in_season_without_number(season: i32) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        parent_index_number: Some(season),
        ..episode()
    }
}

const fn special(index_number: i32) -> AiredEpisodeOrderKey {
    numbered_episode(0, index_number)
}

const fn special_after_season(season: i32, index_number: i32) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        airs_after_season_number: Some(season),
        ..special(index_number)
    }
}

const fn special_after_season_before_episode(
    season: i32,
    index_number: i32,
    episode_number: i32,
) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        airs_before_episode_number: Some(episode_number),
        ..special_after_season(season, index_number)
    }
}

const fn special_before_season(season: i32, index_number: i32) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        airs_before_season_number: Some(season),
        ..special(index_number)
    }
}

const fn special_before_episode(
    season: i32,
    index_number: i32,
    episode_number: i32,
) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        airs_before_episode_number: Some(episode_number),
        ..special_before_season(season, index_number)
    }
}

fn premiered_episode(year: i32, month: u32, day: u32) -> AiredEpisodeOrderKey {
    AiredEpisodeOrderKey {
        parent_index_number: Some(1),
        index_number: Some(1),
        premiere_date: Some(
            Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
                .single()
                .expect("test date must be valid"),
        ),
        ..episode()
    }
}
