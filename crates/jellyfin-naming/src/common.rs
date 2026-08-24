use fancy_regex::RegexBuilder;

use crate::{
    stack::FileStackRule,
    tv::{EpisodeExpression, default_episode_expressions, default_multiple_episode_expressions},
    video::{ExtraRule, ExtraRuleType, ExtraType, Format3dRule, MediaType, StubTypeRule},
};

/// File extensions and flags used while parsing external media filenames.
///
/// The defaults mirror `Emby.Naming.Common.NamingOptions`. Fields remain public
/// because Jellyfin allows server configuration to replace these collections.
#[derive(Clone, Debug)]
pub struct NamingOptions {
    pub audio_file_extensions: Vec<String>,
    pub audio_book_parts_regexes: Vec<fancy_regex::Regex>,
    pub audio_book_name_regexes: Vec<fancy_regex::Regex>,
    pub album_stacking_prefixes: Vec<String>,
    pub subtitle_file_extensions: Vec<String>,
    pub lyric_file_extensions: Vec<String>,
    pub media_flag_delimiters: Vec<char>,
    pub media_forced_flags: Vec<String>,
    pub media_default_flags: Vec<String>,
    pub media_hearing_impaired_flags: Vec<String>,
    pub video_file_extensions: Vec<String>,
    pub video_flag_delimiters: Vec<char>,
    pub stub_file_extensions: Vec<String>,
    pub stub_types: Vec<StubTypeRule>,
    pub clean_date_time_regexes: Vec<fancy_regex::Regex>,
    pub clean_string_regexes: Vec<fancy_regex::Regex>,
    pub format_3d_rules: Vec<Format3dRule>,
    pub episode_expressions: Vec<EpisodeExpression>,
    pub multiple_episode_expressions: Vec<EpisodeExpression>,
    pub video_file_stacking_rules: Vec<FileStackRule>,
    pub video_extra_rules: Vec<ExtraRule>,
}

impl Default for NamingOptions {
    fn default() -> Self {
        Self {
            audio_file_extensions: strings(&[
                ".669", ".3gp", ".aa", ".aac", ".aax", ".ac3", ".act", ".adp", ".adplug", ".adx",
                ".afc", ".amf", ".aif", ".aifc", ".aiff", ".alac", ".amr", ".ape", ".ast", ".au",
                ".awb", ".cda", ".cue", ".dmf", ".dsf", ".dsm", ".dsp", ".dts", ".dvf", ".eac3",
                ".ec3", ".far", ".flac", ".gdm", ".gsm", ".gym", ".hps", ".imf", ".it", ".m15",
                ".m4a", ".m4b", ".mac", ".med", ".mka", ".mmf", ".mod", ".mogg", ".mp2", ".mp3",
                ".mpa", ".mpc", ".mpp", ".mp+", ".msv", ".nmf", ".nsf", ".nsv", ".oga", ".ogg",
                ".okt", ".opus", ".pls", ".ra", ".rf64", ".rm", ".s3m", ".sfx", ".shn", ".sid",
                ".stm", ".strm", ".ult", ".uni", ".vox", ".wav", ".wma", ".wv", ".xm", ".xsp",
                ".ymf",
            ]),
            audio_book_parts_regexes: regexes(&[
                r"ch(?:apter)?[\s_-]?(?P<chapter>[0-9]+)",
                r"p(?:ar)?t[\s_-]?(?P<part>[0-9]+)",
                r"^(?P<chapter>[0-9]+)",
                r"(?P<part>[0-9]+)$",
                r"(?P<chapter>[0-9]+)_(?P<part>[0-9]+)",
                r"dis(?:c|k)[\s_-]?(?P<chapter>[0-9]+)",
            ]),
            audio_book_name_regexes: regexes(&[
                r"^(?P<name>.+?)\s*\(\s*(?P<year>[0-9]{4})\s*\)\s*$",
                r"^\s*(?P<name>[^ ].*?)\s*$",
            ]),
            album_stacking_prefixes: strings(&[
                "cd",
                "digital media",
                "disc",
                "disk",
                "vol",
                "volume",
                "part",
                "act",
            ]),
            subtitle_file_extensions: strings(&[
                ".ass", ".mks", ".sami", ".smi", ".srt", ".ssa", ".sub", ".sup", ".vtt",
            ]),
            lyric_file_extensions: strings(&[".lrc", ".elrc", ".txt"]),
            media_flag_delimiters: vec!['.'],
            media_forced_flags: strings(&["foreign", "forced"]),
            media_default_flags: strings(&["default"]),
            media_hearing_impaired_flags: strings(&["cc", "hi", "sdh"]),
            video_file_extensions: strings(&[
                ".001", ".3g2", ".3gp", ".amv", ".asf", ".asx", ".avi", ".bin", ".bivx", ".divx",
                ".dv", ".dvr-ms", ".f4v", ".fli", ".flv", ".ifo", ".img", ".iso", ".m2t", ".m2ts",
                ".m2v", ".m4v", ".mkv", ".mk3d", ".mov", ".mp4", ".mpe", ".mpeg", ".mpg", ".mts",
                ".mxf", ".nrg", ".nsv", ".nuv", ".ogg", ".ogm", ".ogv", ".pva", ".qt", ".rec",
                ".rm", ".rmvb", ".strm", ".svq3", ".tp", ".ts", ".ty", ".viv", ".vob", ".vp3",
                ".webm", ".wmv", ".wtv", ".xvid",
            ]),
            video_flag_delimiters: vec!['(', ')', '-', '.', '_', '[', ']'],
            stub_file_extensions: strings(&[".disc"]),
            stub_types: vec![
                StubTypeRule::new("dvd", "dvd"),
                StubTypeRule::new("hddvd", "hddvd"),
                StubTypeRule::new("bluray", "bluray"),
                StubTypeRule::new("brrip", "bluray"),
                StubTypeRule::new("bd25", "bluray"),
                StubTypeRule::new("bd50", "bluray"),
                StubTypeRule::new("vhs", "vhs"),
                StubTypeRule::new("HDTV", "tv"),
                StubTypeRule::new("PDTV", "tv"),
                StubTypeRule::new("DSR", "tv"),
            ],
            clean_date_time_regexes: regexes(&[
                r"(.+[^_\,\.\(\)\[\]\-])[_\.\(\)\[\]\-](19[0-9]{2}|20[0-9]{2})(?![0-9]+|\W[0-9]{2}\W[0-9]{2})([ _\,\.\(\)\[\]\-][^0-9]|).*(19[0-9]{2}|20[0-9]{2})*",
                r"(.+[^_\,\.\(\)\[\]\-])[ _\.\(\)\[\]\-]+(19[0-9]{2}|20[0-9]{2})(?![0-9]+|\W[0-9]{2}\W[0-9]{2})([ _\,\.\(\)\[\]\-][^0-9]|).*(19[0-9]{2}|20[0-9]{2})*",
            ]),
            clean_string_regexes: regexes(&[
                r"^\s*(?P<cleaned>.+?)[ _,.()\[\]\-](?:3d|sbs|tab|hsbs|htab|mvc|HDR|HDC|UHD|UltraHD|4k|ac3|dts|custom|dc|divx|divx5|dsr|dsrip|dutch|dvd|dvdrip|dvdscr|dvdscreener|screener|dvdivx|cam|fragment|fs|hdtv|hdrip|hdtvrip|internal|limited|multi|subs|ntsc|ogg|ogm|pal|pdtv|proper|repack|rerip|retail|cd[1-9]|r5|bd5|bd|se|svcd|swedish|german|read\.nfo|nfofix|unrated|ws|telesync|ts|telecine|tc|brrip|bdrip|480p|480i|576p|576i|720p|720i|1080p|1080i|2160p|hrhd|hrhdtv|hddvd|bluray|blu-ray|x264|x265|h264|h265|xvid|xvidvd|xxx|www\.www|AAC|DTS)(?:[ _,.()\[\]\-]|$)",
                r"^\s*(?P<cleaned>.+?)((\s*\[[^\]]+\]\s*)+)(\.[^\s]+)?$",
                r"^\s*(?P<cleaned>.+?)\WE[0-9]+(?:-|~)E?[0-9]+(?:\W|$)",
                r"^\s*\[[^\]]+\](?!\.\w+$)\s*(?P<cleaned>.+)",
                r"^\s*(?P<cleaned>.+?)\s+-\s+[0-9]+\s*$",
                r"^\s*(?P<cleaned>.+?)(?:(?:[-._ ](?:trailer|sample))|-(?:scene|clip|behindthescenes|deleted|deletedscene|featurette|short|interview|other|extra))$",
            ]),
            format_3d_rules: vec![
                Format3dRule::with_preceding("hsbs", "3d"),
                Format3dRule::with_preceding("sbs", "3d"),
                Format3dRule::with_preceding("htab", "3d"),
                Format3dRule::with_preceding("tab", "3d"),
                Format3dRule::new("fsbs"),
                Format3dRule::new("hsbs"),
                Format3dRule::new("sbs"),
                Format3dRule::new("ftab"),
                Format3dRule::new("htab"),
                Format3dRule::new("tab"),
                Format3dRule::new("sbs3d"),
                Format3dRule::new("mvc"),
            ],
            episode_expressions: default_episode_expressions(),
            multiple_episode_expressions: default_multiple_episode_expressions(),
            video_file_stacking_rules: vec![
                FileStackRule::new(
                    r"^(?P<filename>.*?)(?P<separator>[ _.-]+|[\]\)\}])[\(\[]?(?P<parttype>cd|dvd|part|pt|dis[ck])[ _.-]*(?P<number>[0-9]+)[\)\]]?(?:\.[^.]+)?$",
                    true,
                ),
                FileStackRule::new(
                    r"^(?P<filename>.*?)(?P<separator>[ _.-]+|[\]\)\}])[\(\[]?(?P<parttype>cd|dvd|part|pt|dis[ck])[ _.-]*(?P<number>[a-d])[\)\]]?(?:\.[^.]+)?$",
                    false,
                ),
            ],
            video_extra_rules: default_video_extra_rules(),
        }
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn default_video_extra_rules() -> Vec<ExtraRule> {
    use ExtraRuleType::{DirectoryName, Filename, Suffix};
    use ExtraType::{
        BehindTheScenes, Clip, DeletedScene, Featurette, Interview, Sample, Scene, Short,
        ThemeSong, ThemeVideo, Trailer, Unknown,
    };

    let directories = [
        (Trailer, "trailers", MediaType::Video),
        (ThemeVideo, "backdrops", MediaType::Video),
        (ThemeSong, "theme-music", MediaType::Audio),
        (BehindTheScenes, "behind the scenes", MediaType::Video),
        (DeletedScene, "deleted scenes", MediaType::Video),
        (Interview, "interviews", MediaType::Video),
        (Scene, "scenes", MediaType::Video),
        (Sample, "samples", MediaType::Video),
        (Short, "shorts", MediaType::Video),
        (Featurette, "featurettes", MediaType::Video),
        (Unknown, "extras", MediaType::Video),
        (Unknown, "extra", MediaType::Video),
        (Unknown, "other", MediaType::Video),
        (Clip, "clips", MediaType::Video),
    ];
    let filenames = [
        (Trailer, "trailer", MediaType::Video),
        (Sample, "sample", MediaType::Video),
        (ThemeSong, "theme", MediaType::Audio),
    ];
    let suffixes = [
        (Trailer, "-trailer"),
        (Trailer, ".trailer"),
        (Trailer, "_trailer"),
        (Trailer, "- trailer"),
        (Sample, "-sample"),
        (Sample, ".sample"),
        (Sample, "_sample"),
        (Sample, "- sample"),
        (Scene, "-scene"),
        (Clip, "-clip"),
        (Interview, "-interview"),
        (BehindTheScenes, "-behindthescenes"),
        (DeletedScene, "-deleted"),
        (DeletedScene, "-deletedscene"),
        (Featurette, "-featurette"),
        (Short, "-short"),
        (Unknown, "-extra"),
        (Unknown, "-other"),
    ];

    directories
        .into_iter()
        .map(|(extra_type, token, media_type)| {
            ExtraRule::new(extra_type, DirectoryName, token, media_type)
        })
        .chain(
            filenames
                .into_iter()
                .map(|(extra_type, token, media_type)| {
                    ExtraRule::new(extra_type, Filename, token, media_type)
                }),
        )
        .chain(
            suffixes.into_iter().map(|(extra_type, token)| {
                ExtraRule::new(extra_type, Suffix, token, MediaType::Video)
            }),
        )
        .collect()
}

fn regexes(values: &[&str]) -> Vec<fancy_regex::Regex> {
    values
        .iter()
        .map(|expression| {
            RegexBuilder::new(expression)
                .case_insensitive(true)
                .build()
                .expect("built-in naming expression must be valid")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::NamingOptions;

    #[test]
    fn defaults_match_external_file_options() {
        let options = NamingOptions::default();
        assert_eq!(options.media_flag_delimiters, ['.']);
        assert_eq!(options.media_forced_flags, ["foreign", "forced"]);
        assert_eq!(options.media_default_flags, ["default"]);
        assert_eq!(options.media_hearing_impaired_flags, ["cc", "hi", "sdh"]);
        assert!(options.audio_file_extensions.contains(&".mp3".to_owned()));
        assert_eq!(
            options.album_stacking_prefixes,
            [
                "cd",
                "digital media",
                "disc",
                "disk",
                "vol",
                "volume",
                "part",
                "act"
            ]
        );
        assert!(
            options
                .subtitle_file_extensions
                .contains(&".srt".to_owned())
        );
        assert_eq!(options.lyric_file_extensions, [".lrc", ".elrc", ".txt"]);
        assert!(!options.clean_date_time_regexes.is_empty());
        assert!(!options.clean_string_regexes.is_empty());
        assert!(options.video_file_extensions.contains(&".mkv".to_owned()));
        assert_eq!(options.stub_file_extensions, [".disc"]);
        assert!(!options.episode_expressions.is_empty());
        assert!(!options.multiple_episode_expressions.is_empty());
    }
}
