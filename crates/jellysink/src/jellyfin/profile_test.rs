use super::*;

#[test]
fn direct_play_profiles_have_no_codec_list() {
    let profile = device_profile();
    for entry in profile["DirectPlayProfiles"].as_array().unwrap() {
        assert!(entry.get("Container").is_none());
        assert!(entry.get("VideoCodec").is_none());
        assert!(entry.get("AudioCodec").is_none());
    }
}
