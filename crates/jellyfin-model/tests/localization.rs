use jellyfin_model::LocalizationOption;
use serde_json::json;

#[test]
fn localization_option_uses_the_official_pascal_case_contract() {
    let option = LocalizationOption {
        name: "English".to_owned(),
        value: "en-US".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(option).unwrap(),
        json!({ "Name": "English", "Value": "en-US" })
    );
}
