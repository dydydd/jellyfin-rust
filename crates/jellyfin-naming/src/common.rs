/// File extensions and flags used while parsing external media filenames.
///
/// The defaults mirror `Emby.Naming.Common.NamingOptions`. Fields remain public
/// because Jellyfin allows server configuration to replace these collections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamingOptions {
    pub audio_file_extensions: Vec<String>,
    pub subtitle_file_extensions: Vec<String>,
    pub lyric_file_extensions: Vec<String>,
    pub media_flag_delimiters: Vec<char>,
    pub media_forced_flags: Vec<String>,
    pub media_default_flags: Vec<String>,
    pub media_hearing_impaired_flags: Vec<String>,
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
            subtitle_file_extensions: strings(&[
                ".ass", ".mks", ".sami", ".smi", ".srt", ".ssa", ".sub", ".sup", ".vtt",
            ]),
            lyric_file_extensions: strings(&[".lrc", ".elrc", ".txt"]),
            media_flag_delimiters: vec!['.'],
            media_forced_flags: strings(&["foreign", "forced"]),
            media_default_flags: strings(&["default"]),
            media_hearing_impaired_flags: strings(&["cc", "hi", "sdh"]),
        }
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
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
        assert!(
            options
                .subtitle_file_extensions
                .contains(&".srt".to_owned())
        );
        assert_eq!(options.lyric_file_extensions, [".lrc", ".elrc", ".txt"]);
    }
}
