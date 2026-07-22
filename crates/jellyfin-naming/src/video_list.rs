use std::{cmp::Ordering, sync::LazyLock};

use regex::{Regex, RegexBuilder};

use crate::{
    EpisodePathParser, ExtraType, NamingOptions, StackFileInfo, StackResolver, VideoFileInfo,
};

static RESOLUTION: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"([0-9]{3,4})[ip]")
        .case_insensitive(true)
        .build()
        .expect("resolution expression must be valid")
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionType {
    Movies,
    TvShows,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoInfo {
    pub name: String,
    pub year: Option<u16>,
    pub files: Vec<VideoFileInfo>,
    pub alternate_versions: Vec<VideoInfo>,
    pub extra_type: Option<ExtraType>,
}

pub struct VideoListResolver {
    options: NamingOptions,
}

impl VideoListResolver {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn resolve(&self, videos: &[VideoFileInfo]) -> Vec<VideoInfo> {
        self.resolve_with_options(videos, true, None)
    }

    #[must_use]
    pub fn resolve_with_options(
        &self,
        videos: &[VideoFileInfo],
        support_multi_version: bool,
        collection_type: Option<CollectionType>,
    ) -> Vec<VideoInfo> {
        let stack_files = videos
            .iter()
            .filter(|video| video.extra_type.is_none())
            .map(|video| StackFileInfo::new(&video.path, video.is_directory))
            .collect::<Vec<_>>();
        let stacks = StackResolver::resolve(&stack_files, &self.options);
        let mut consumed = Vec::new();
        let mut media = Vec::new();
        for stack in stacks {
            let files = stack
                .files
                .iter()
                .filter_map(|path| {
                    videos
                        .iter()
                        .find(|video| video.path.eq_ignore_ascii_case(path))
                        .cloned()
                })
                .collect::<Vec<_>>();
            consumed.extend(stack.files);
            media.push(VideoInfo {
                name: stack.name,
                year: files.first().and_then(|file| file.year),
                files,
                alternate_versions: Vec::new(),
                extra_type: None,
            });
        }
        let mut extras = Vec::new();
        for video in videos {
            if consumed
                .iter()
                .any(|path| path.eq_ignore_ascii_case(&video.path))
            {
                continue;
            }
            let info = VideoInfo {
                name: video.name.clone(),
                year: video.year,
                files: vec![video.clone()],
                alternate_versions: Vec::new(),
                extra_type: video.extra_type,
            };
            if video.extra_type.is_some() {
                extras.push(info);
            } else {
                media.push(info);
            }
        }

        if support_multi_version {
            media = if collection_type == Some(CollectionType::TvShows) {
                self.group_episodes(media)
            } else {
                self.group_movies(media)
            };
        }
        media.extend(extras);
        media
    }

    fn group_movies(&self, videos: Vec<VideoInfo>) -> Vec<VideoInfo> {
        if videos.len() < 2 {
            return videos;
        }
        let Some(first) = videos.first().and_then(|video| video.files.first()) else {
            return videos;
        };
        let folder = parent_name(&first.path).to_owned();
        if folder.chars().count() <= 1 || !have_same_year(&videos) {
            return videos;
        }
        let mut primary_path = None;
        for video in &videos {
            let stem = file_stem(&video.files[0]);
            if !eligible_for_multi_version(&folder, stem, &self.options) {
                return videos;
            }
            if stem == folder {
                primary_path = video.files.first().map(|file| file.path.clone());
            }
        }
        vec![organize_versions(videos, primary_path, Some(folder))]
    }

    fn group_episodes(&self, videos: Vec<VideoInfo>) -> Vec<VideoInfo> {
        if videos.len() < 2 {
            return videos;
        }
        let parser = EpisodePathParser::new(self.options.clone());
        let mut groups: Vec<(String, Vec<VideoInfo>)> = Vec::new();
        let mut result = Vec::new();
        for video in videos {
            let parsed = parser.parse(&video.files[0].path, false);
            let key = if parsed.is_by_date {
                match (parsed.year, parsed.month, parsed.day) {
                    (Some(year), Some(month), Some(day)) => {
                        Some(format!("D{year}{month:02}{day:02}"))
                    }
                    _ => None,
                }
            } else {
                parsed
                    .episode_number
                    .map(|episode| format!("S{}E{episode}", parsed.season_number.unwrap_or(0)))
            };
            let Some(key) = key else {
                result.push(video);
                continue;
            };
            if let Some((_, values)) = groups
                .iter_mut()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&key))
            {
                values.push(video);
            } else {
                groups.push((key, vec![video]));
            }
        }
        for (_, group) in groups {
            if group.len() == 1 {
                result.extend(group);
            } else {
                result.push(organize_versions(group, None, None));
            }
        }
        result
    }
}

fn have_same_year(videos: &[VideoInfo]) -> bool {
    videos
        .first()
        .is_none_or(|first| videos.iter().all(|video| video.year == first.year))
}

fn eligible_for_multi_version(folder: &str, stem: &str, options: &NamingOptions) -> bool {
    let Some(suffix) = strip_prefix_ignore_ascii_case(stem, folder) else {
        return false;
    };
    let suffix = suffix.trim();
    let cleaned = crate::VideoResolver::try_clean_string(Some(suffix), options);
    let suffix = cleaned.as_deref().unwrap_or(suffix).trim();
    suffix.is_empty()
        || suffix.starts_with(['-', '_', '.'])
        || (suffix.starts_with('[') && suffix.find(']').is_some())
}

fn organize_versions(
    mut videos: Vec<VideoInfo>,
    primary_override: Option<String>,
    name_override: Option<String>,
) -> VideoInfo {
    videos.sort_by(compare_versions);
    let primary_index = primary_override
        .and_then(|path| {
            videos
                .iter()
                .position(|video| video.files.first().is_some_and(|file| file.path == path))
        })
        .or_else(|| videos.iter().position(|video| video.files.len() > 1))
        .unwrap_or(0);
    let mut primary = videos.remove(primary_index);
    primary.alternate_versions = videos;
    if let Some(name) = name_override {
        primary.name = name;
    }
    primary
}

fn compare_versions(left: &VideoInfo, right: &VideoInfo) -> Ordering {
    let left_name = file_stem(&left.files[0]);
    let right_name = file_stem(&right.files[0]);
    match (resolution(left_name), resolution(right_name)) {
        (Some(left), Some(right)) => right
            .cmp(&left)
            .then_with(|| natural_cmp(left_name, right_name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => natural_cmp(left_name, right_name),
    }
}

fn resolution(value: &str) -> Option<u32> {
    RESOLUTION.captures(value)?.get(1)?.as_str().parse().ok()
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let (mut left_index, mut right_index) = (0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index].is_ascii_digit() && right[right_index].is_ascii_digit() {
            let left_end = digit_end(left, left_index);
            let right_end = digit_end(right, right_index);
            let left_number = &left[left_index..left_end];
            let right_number = &right[right_index..right_end];
            let ordering = left_number
                .len()
                .cmp(&right_number.len())
                .then_with(|| left_number.cmp(right_number));
            if ordering != Ordering::Equal {
                return ordering;
            }
            left_index = left_end;
            right_index = right_end;
        } else {
            let ordering = left[left_index]
                .to_ascii_lowercase()
                .cmp(&right[right_index].to_ascii_lowercase());
            if ordering != Ordering::Equal {
                return ordering;
            }
            left_index += 1;
            right_index += 1;
        }
    }
    left.len().cmp(&right.len())
}

fn digit_end(value: &[u8], start: usize) -> usize {
    value[start..]
        .iter()
        .position(|character| !character.is_ascii_digit())
        .map_or(value.len(), |offset| start + offset)
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))?;
    value.get(prefix.len()..)
}

fn file_stem(video: &VideoFileInfo) -> &str {
    let name = file_name(&video.path);
    if video.is_directory {
        name
    } else {
        name.rfind('.').map_or(name, |index| &name[..index])
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent_path(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |index| &path[..index])
}

fn parent_name(path: &str) -> &str {
    file_name(parent_path(path))
}
