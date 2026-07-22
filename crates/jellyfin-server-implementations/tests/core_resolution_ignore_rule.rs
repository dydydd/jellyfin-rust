use jellyfin_naming::{ExtraRule, ExtraRuleType, ExtraType, MediaType, NamingOptions};
use jellyfin_server_implementations::{
    CoreResolutionIgnoreRule, ResolutionFileSystemEntry, ResolutionParentContext,
    ResolutionParentKind,
};

#[test]
fn application_folder_entries_are_never_ignored() {
    let rule = rule();

    assert!(!rule.should_ignore(
        &entry("/server/root/extras", true),
        ResolutionParentContext::None,
    ));
    assert!(!rule.should_ignore(
        &entry("/server/root/small.jpg", false),
        ResolutionParentContext::None,
    ));
}

#[test]
fn server_root_uses_official_case_sensitive_substring_semantics() {
    let rule = rule();

    assert!(!rule.should_ignore(
        &entry("/media/server/rooted/small.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
    assert!(!rule.should_ignore(
        &entry("/media/archive/server/root/small.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
    assert!(rule.should_ignore(
        &entry("/SERVER/ROOT/small.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
    assert!(rule.should_ignore(
        &entry(r"C:\server\root\small.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
}

#[test]
fn top_level_directories_are_not_ignored() {
    let rule = rule();

    assert!(!rule.should_ignore(
        &entry("Series/Extras", true),
        parent(ResolutionParentKind::AggregateFolder),
    ));
    assert!(!rule.should_ignore(
        &entry("Series/Extras/Extras", true),
        ResolutionParentContext::top_parent(ResolutionParentKind::BaseItem),
    ));
}

#[test]
fn fixed_ignore_patterns_run_before_parent_exemptions() {
    let rule = rule();

    assert!(!rule.should_ignore(
        &entry("/Media/big.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
    assert!(rule.should_ignore(
        &entry("/Media/small.jpg", false),
        parent(ResolutionParentKind::BaseItem),
    ));
    assert!(rule.should_ignore(
        &entry(r"C:\Media\THUMBS.DB", false),
        ResolutionParentContext::None,
    ));
}

#[test]
fn extras_folder_names_are_derived_from_naming_options() {
    let rule = rule();

    for path in ["/Movies/Up/extras", r"C:\Movies\Up\EXTRAS"] {
        let file = entry(path, true);
        assert!(!rule.should_ignore(&file, parent(ResolutionParentKind::AggregateFolder),));
        assert!(!rule.should_ignore(&file, parent(ResolutionParentKind::UserRootFolder),));
        assert!(!rule.should_ignore(&file, ResolutionParentContext::None));
        assert!(rule.should_ignore(&file, parent(ResolutionParentKind::BaseItem),));
        assert!(rule.should_ignore(&file, parent(ResolutionParentKind::Folder),));
    }

    assert!(rule.should_ignore(
        &entry("/Movies/Up/theme-music", true),
        parent(ResolutionParentKind::Folder),
    ));
    assert!(!rule.should_ignore(
        &entry("/Movies/Up/not-extras", true),
        parent(ResolutionParentKind::Folder),
    ));
}

#[test]
fn only_case_exact_theme_stem_with_audio_extension_is_ignored() {
    let rule = rule();
    let parent = parent(ResolutionParentKind::BaseItem);

    assert!(!rule.should_ignore(&entry("/Movies/Up/intro.mp3", false), parent));
    assert!(rule.should_ignore(&entry("/Movies/Up/theme.mp3", false), parent));
    assert!(rule.should_ignore(&entry("/Movies/Up/theme.MP3", false), parent));
    assert!(!rule.should_ignore(&entry("/Movies/Up/Theme.mp3", false), parent));
    assert!(!rule.should_ignore(&entry("/Movies/Up/theme.txt", false), parent));
}

#[test]
fn theme_song_stem_is_not_configured_by_extra_filename_rules() {
    let mut options = NamingOptions::default();
    options.video_extra_rules.retain(|rule| {
        rule.extra_type != ExtraType::ThemeSong || rule.rule_type != ExtraRuleType::Filename
    });
    options.video_extra_rules.push(ExtraRule::new(
        ExtraType::ThemeSong,
        ExtraRuleType::Filename,
        "custom-theme",
        MediaType::Audio,
    ));
    let rule = CoreResolutionIgnoreRule::new(options, "/server/root");
    let parent = parent(ResolutionParentKind::BaseItem);

    assert!(rule.should_ignore(&entry("/Movies/Up/theme.mp3", false), parent));
    assert!(!rule.should_ignore(&entry("/Movies/Up/custom-theme.mp3", false), parent));
}

fn rule() -> CoreResolutionIgnoreRule {
    CoreResolutionIgnoreRule::new(NamingOptions::default(), "/server/root")
}

fn entry(path: &str, is_directory: bool) -> ResolutionFileSystemEntry {
    ResolutionFileSystemEntry::new(path, is_directory)
}

const fn parent(kind: ResolutionParentKind) -> ResolutionParentContext {
    ResolutionParentContext::item(kind)
}
